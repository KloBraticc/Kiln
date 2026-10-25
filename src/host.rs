use std::collections::{HashMap, HashSet};

use crate::codegen::{CompiledMethod, Compiler, Helpers, MethodKey};
use crate::interpreter::{Fault, Interpreter, V};
use crate::loader::AssemblySet;
use crate::metadata::TableId;
use crate::objects::{ObjectModel, RuntimeType};
use crate::reachability::Reachability;

pub fn entry_point(set: &AssemblySet, application: usize) -> Option<MethodKey> {
    let token = set.modules[application].image.cli.entry_point;
    (token >> 24 == 0x06 && token & 0x00FF_FFFF != 0).then(|| MethodKey {
        module: application,
        row: token & 0x00FF_FFFF,
    })
}

pub fn reachable(set: &AssemblySet, application: usize) -> HashSet<MethodKey> {
    let mut analysis = Reachability::new(set);
    analysis.add_application(application);
    analysis.run();
    analysis
        .methods
        .iter()
        .map(|method| MethodKey {
            module: method.module,
            row: method.row,
        })
        .collect()
}

pub fn compile(
    set: &AssemblySet,
    helpers: &Helpers,
    interpreter: &Interpreter<'_>,
    wanted: &HashSet<MethodKey>,
) -> HashMap<MethodKey, CompiledMethod> {
    let compiler = Compiler::new(set, helpers);
    let mut methods = HashMap::new();
    let mut failed = HashSet::new();
    let mut queue: Vec<MethodKey> = wanted.iter().cloned().collect();
    while let Some(key) = queue.pop() {
        if methods.contains_key(&key) || failed.contains(&key) || interpreter.has_native(&key) {
            continue;
        }
        if !compilable(set, &key) {
            failed.insert(key);
            continue;
        }
        match compiler.compile(&key) {
            Ok(method) => {
                queue.extend(method.callees.iter().cloned());
                methods.insert(key, method);
            }
            Err(_) => {
                failed.insert(key);
            }
        }
    }
    loop {
        let broken: Vec<MethodKey> = methods
            .iter()
            .filter(|(_, method)| {
                method
                    .callees
                    .iter()
                    .any(|callee| !methods.contains_key(callee) && interpreter.stub_for(callee).is_none())
            })
            .map(|(key, _)| key.clone())
            .collect();
        if broken.is_empty() {
            return methods;
        }
        for key in broken {
            methods.remove(&key);
        }
    }
}

fn compilable(set: &AssemblySet, key: &MethodKey) -> bool {
    let metadata = &set.modules[key.module].metadata;
    let rva = metadata.cell(TableId::MethodDef, key.row, 0).unwrap_or(0);
    let implementation = metadata.cell(TableId::MethodDef, key.row, 1).unwrap_or(0);
    rva != 0 && implementation & 0x3 == 0 && crate::codegen::method_types(set, key).is_ok()
}

pub fn start(model: &ObjectModel<'_>) {
    let state = crate::runtime::install();
    if let Ok(record) = state.type_record(model, RuntimeType::String) {
        state.string_type = record;
    }
    state.empty_string(model);
}

pub fn run_main(interpreter: &Interpreter<'_>, entry: &MethodKey, arguments: &[String]) -> i32 {
    let parameters = interpreter.parameter_count(entry);
    let values = if parameters == Some(1) {
        match interpreter.string_array(arguments) {
            Ok(array) => vec![V::O(array)],
            Err(fault) => return report(interpreter, fault),
        }
    } else {
        Vec::new()
    };
    interpreter.budget.set(u64::MAX);
    if interpreter.has_attribute(entry, "STAThreadAttribute") {
        crate::pinvoke::enter_apartment(0);
    }
    match interpreter.invoke(entry, &values) {
        Ok(Some(value)) => value.int() as i32,
        Ok(None) => 0,
        Err(fault) => report(interpreter, fault),
    }
}

pub(crate) fn report(interpreter: &Interpreter<'_>, fault: Fault) -> i32 {
    match fault {
        Fault::Exception(object) => {
            match interpreter.exception_message(object) {
                Some(message) => eprintln!("Unhandled exception. {}: {message}", interpreter.exception_type(object)),
                None => eprintln!("Unhandled exception. {}", interpreter.exception_type(object)),
            }
            -532462766
        }
        Fault::Unsupported(reason) => {
            eprintln!("Kiln cannot run this application yet: {reason}");
            134
        }
    }
}

pub fn config_properties(runtime_config: &str) -> Vec<(String, String)> {
    let Some(start) = runtime_config.find("\"configProperties\"") else {
        return Vec::new();
    };
    let mut chars = runtime_config[start + 18..].chars().skip_while(|c| *c != '{').skip(1).peekable();
    let mut properties = Vec::new();
    let read_string = |chars: &mut std::iter::Peekable<_>| {
        let mut text = String::new();
        let mut escaped = false;
        for c in chars.by_ref() {
            match (escaped, c) {
                (true, 'n') => text.push('\n'),
                (true, 't') => text.push('\t'),
                (true, other) => text.push(other),
                (false, '\\') => {
                    escaped = true;
                    continue;
                }
                (false, '"') => break,
                (false, other) => text.push(other),
            }
            escaped = false;
        }
        text
    };
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        match chars.next() {
            Some('"') => {}
            _ => return properties,
        }
        let key = read_string(&mut chars);
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ':') {
            chars.next();
        }
        let value = if chars.peek() == Some(&'"') {
            chars.next();
            read_string(&mut chars)
        } else {
            let mut text = String::new();
            while let Some(&c) = chars.peek() {
                if c == ',' || c == '}' || c.is_whitespace() {
                    break;
                }
                text.push(c);
                chars.next();
            }
            text
        };
        properties.push((key, value));
    }
}

pub fn default_properties(base_directory: &str) -> Vec<(String, String)> {
    vec![
        ("APP_CONTEXT_BASE_DIRECTORY".into(), base_directory.into()),
        ("System.Diagnostics.Tracing.EventSource.IsSupported".into(), "false".into()),
        ("System.Diagnostics.Debugger.IsSupported".into(), "false".into()),
        ("System.Runtime.CompilerServices.RuntimeFeature.IsDynamicCodeSupported".into(), "false".into()),
    ]
}

pub fn configure(interpreter: &Interpreter<'_>, properties: &[(String, String)]) -> Result<(), String> {
    let set_data = interpreter
        .find_method("System", "AppContext", "SetData", 2)
        .ok_or("AppContext.SetData is missing")?;
    for (name, value) in properties {
        let name = crate::runtime::new_string(&name.encode_utf16().collect::<Vec<_>>());
        let value = crate::runtime::new_string(&value.encode_utf16().collect::<Vec<_>>());
        interpreter
            .invoke(&set_data, &[V::O(name), V::O(value)])
            .map_err(|fault| match fault {
                Fault::Exception(object) => interpreter.exception_type(object),
                Fault::Unsupported(reason) => reason,
            })?;
    }
    Ok(())
}

pub fn closure(set: &AssemblySet, application: usize) -> Vec<usize> {
    let mut order = vec![application];
    let mut seen: HashSet<usize> = order.iter().copied().collect();
    let mut index = 0;
    while index < order.len() {
        let metadata = &set.modules[order[index]].metadata;
        for row in 1..=metadata.rows(TableId::AssemblyRef) {
            let Ok(name) = metadata.string_cell(TableId::AssemblyRef, row, 6) else {
                continue;
            };
            if let Some(found) = set.find_assembly(name)
                && seen.insert(found)
            {
                order.push(found);
            }
        }
        index += 1;
    }
    order
}

pub fn launch(
    set: &AssemblySet,
    application: usize,
    properties: &[(String, String)],
    arguments: &[String],
    methods: impl FnOnce(&Interpreter<'_>) -> HashMap<MethodKey, CompiledMethod>,
) -> i32 {
    let Some(entry) = entry_point(set, application) else {
        eprintln!("{} has no entry point", set.modules[application].name);
        return 2;
    };
    let model = ObjectModel::new(set);
    start(&model);
    let interpreter = Interpreter::new(&model, None);
    interpreter.install_bridge();
    interpreter.set_entry_module(application);
    let methods = methods(&interpreter);
    let image = match crate::jit::Image::link_with_fallback(
        &methods,
        &model,
        &|key| interpreter.stub_for(key),
        &|module, token| interpreter.array_record_for(module, token),
    ) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("Kiln could not link the application: {error}");
            return 134;
        }
    };
    interpreter.attach(&image);
    if let Err(error) = configure(&interpreter, properties) {
        eprintln!("Kiln could not configure the application: {error}");
        return 134;
    }
    run_main(&interpreter, &entry, arguments)
}
