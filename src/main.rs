use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use kiln::{AssemblyReport, Error};

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("inspect") if arguments.len() > 1 => inspect(&arguments[1..]),
        Some("resolve") if arguments.len() > 1 => resolve(&arguments[1..]),
        Some("layout") if arguments.len() > 1 => layout(&arguments[1..]),
        Some("offsets") if arguments.len() > 1 => offsets(&arguments[1..]),
        Some("ir") if arguments.len() > 1 => ir(&arguments[1..]),
        Some("disasm") => disasm(&arguments[1..]),
        Some("jit-cases") if arguments.len() > 2 => jit_cases(&arguments[1..]),
        Some("jit-check") if arguments.len() > 2 => {
            let rest = arguments[1..].to_vec();
            std::thread::Builder::new()
                .stack_size(512 * 1024 * 1024)
                .spawn(move || jit_check(&rest))
                .map(|worker| worker.join().unwrap_or(ExitCode::from(101)))
                .unwrap_or(ExitCode::from(101))
        }
        Some("--version") => {
            println!("kiln {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "usage: kiln inspect <assembly or folder>...
       kiln resolve <assembly or folder>..."
            );
            ExitCode::from(2)
        }
    }
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                collect(&entry.path(), out);
            }
        }
    } else if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dll") || e.eq_ignore_ascii_case("exe"))
    {
        out.push(path.to_path_buf());
    }
}

fn inspect(arguments: &[String]) -> ExitCode {
    let mut files = Vec::new();
    for argument in arguments {
        collect(Path::new(argument), &mut files);
    }
    files.sort();

    let started = Instant::now();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(PathBuf, Result<AssemblyReport, Error>)>> =
        Mutex::new(Vec::with_capacity(files.len()));
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(index) else { break };
                    let outcome = std::fs::read(path)
                        .map_err(|e| Error::Io(e.to_string()))
                        .and_then(|data| kiln::inspect(&data));
                    results
                        .lock()
                        .expect("results lock")
                        .push((path.clone(), outcome));
                }
            });
        }
    });
    let elapsed = started.elapsed();

    let mut results = results.into_inner().expect("results lock");
    results.sort_by(|a, b| a.0.cmp(&b.0));

    let (mut managed, mut native, mut broken) = (0usize, 0usize, 0usize);
    let (mut types, mut bodies, mut instructions, mut failures, mut signatures) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    for (path, outcome) in &results {
        match outcome {
            Ok(report) => {
                managed += 1;
                types += u64::from(report.types);
                bodies += u64::from(report.bodies);
                instructions += report.stats.instructions;
                signatures += report.signatures;
                failures += report.failures.len() as u64;
                if arguments.len() == 1 && results.len() == 1 || !report.failures.is_empty() {
                    println!(
                        "{}  {}  metadata {}{}  types {}  methods {}  bodies {}  instructions {}",
                        path.display(),
                        report.name.as_deref().unwrap_or("(module)"),
                        report.metadata_version,
                        if report.ready_to_run {
                            "  ReadyToRun"
                        } else {
                            ""
                        },
                        report.types,
                        report.methods,
                        report.bodies,
                        report.stats.instructions
                    );
                }
                for failure in &report.failures {
                    println!("  FAIL {failure}");
                }
            }
            Err(Error::NotManaged) => native += 1,
            Err(error) => {
                broken += 1;
                println!("{}  ERROR {error}", path.display());
            }
        }
    }

    println!(
        "{managed} managed assemblies, {native} native images skipped, {broken} unreadable, {types} types, {bodies} method bodies, {instructions} instructions, {signatures} signatures, {failures} failures in {:.2?}",
        elapsed
    );
    if broken == 0 && failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn resolve(arguments: &[String]) -> ExitCode {
    let mut files = Vec::new();
    for argument in arguments {
        let mut found = Vec::new();
        collect(Path::new(argument), &mut found);
        found.sort();
        files.extend(found);
    }

    let started = Instant::now();
    let mut set = kiln::loader::AssemblySet::default();
    let mut unreadable = 0usize;
    for file in &files {
        if let Err(error) = set.add(file) {
            unreadable += 1;
            println!("{}  ERROR {error}", file.display());
        }
    }
    let loaded = started.elapsed();

    let mut resolved = 0u64;
    let mut missing_assemblies: std::collections::BTreeMap<String, u64> = Default::default();
    let mut failures = 0u64;
    for (index, module) in set.modules.iter().enumerate() {
        for row in 1..=module.metadata.rows(kiln::metadata::TableId::TypeRef) {
            match set.resolve_type_ref(index, row) {
                Ok(_) => resolved += 1,
                Err(kiln::loader::ResolveError::MissingAssembly(name)) => {
                    *missing_assemblies.entry(name).or_default() += 1
                }
                Err(error) => {
                    failures += 1;
                    println!("  FAIL {} TypeRef {row}: {error:?}", module.name);
                }
            }
        }
    }

    let mut members = 0u64;
    let mut array_members = 0u64;
    for (index, module) in set.modules.iter().enumerate() {
        for row in 1..=module.metadata.rows(kiln::metadata::TableId::MemberRef) {
            match set.resolve_member_ref(index, row) {
                Ok(kiln::members::Member::ArrayMethod) => array_members += 1,
                Ok(_) => members += 1,
                Err(kiln::loader::ResolveError::MissingAssembly(name)) => {
                    *missing_assemblies.entry(name).or_default() += 1
                }
                Err(error) => {
                    failures += 1;
                    println!("  FAIL {} MemberRef {row}: {error:?}", module.name);
                }
            }
        }
    }

    for (name, count) in &missing_assemblies {
        println!("  not in the set: {name} ({count} references)");
    }
    println!(
        "{} assemblies loaded in {:.2?}, {unreadable} unreadable, {resolved} type references and {members} member references resolved, {array_members} array methods, {} point outside the set, {failures} failures, total {:.2?}",
        set.modules.len(),
        loaded,
        missing_assemblies.values().sum::<u64>(),
        started.elapsed()
    );
    if failures == 0 && unreadable == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn load_set(arguments: &[String]) -> kiln::loader::AssemblySet {
    let mut set = kiln::loader::AssemblySet::default();
    for argument in arguments {
        let mut found = Vec::new();
        collect(Path::new(argument), &mut found);
        found.sort();
        for file in found {
            if let Err(error) = set.add(&file) {
                eprintln!("{}  ERROR {error}", file.display());
            }
        }
    }
    set
}

fn layout(arguments: &[String]) -> ExitCode {
    let set = load_set(arguments);
    let mut lines = Vec::new();
    let mut failures = 0u64;
    for (index, module) in set.modules.iter().enumerate() {
        let (Ok(byref_like), Ok(generic)) = (
            set.types_with_attribute(
                index,
                "System.Runtime.CompilerServices",
                "IsByRefLikeAttribute",
            ),
            set.generic_definitions(index),
        ) else {
            failures += 1;
            continue;
        };
        for row in 1..=module.metadata.rows(kiln::metadata::TableId::TypeDef) {
            if byref_like.contains(&row) || generic.contains(&row) {
                continue;
            }
            let handle = kiln::loader::TypeHandle { module: index, row };
            match set.is_value_type(handle) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => continue,
            }
            let name = set.full_type_name(handle).unwrap_or_default();
            match set.value_type_layout(handle) {
                Ok(layout) => lines.push(format!("{}|{name}|{}", module.name, layout.size)),
                Err(error) => {
                    failures += 1;
                    eprintln!("  FAIL {}|{name}: {error:?}", module.name);
                }
            }
        }
    }
    lines.sort();
    for line in &lines {
        println!("{line}");
    }
    eprintln!("{} value types laid out, {failures} failures", lines.len());
    ExitCode::SUCCESS
}

fn offsets(arguments: &[String]) -> ExitCode {
    let set = load_set(arguments);
    let mut lines = Vec::new();
    let mut failures = 0u64;
    for (index, module) in set.modules.iter().enumerate() {
        let Ok(generic) = set.generic_definitions(index) else {
            failures += 1;
            continue;
        };
        for row in 1..=module.metadata.rows(kiln::metadata::TableId::TypeDef) {
            let flags = module
                .metadata
                .cell(kiln::metadata::TableId::TypeDef, row, 0)
                .unwrap_or(0);
            if generic.contains(&row) || flags & 0x20 != 0 {
                continue;
            }
            let handle = kiln::loader::TypeHandle { module: index, row };
            if !matches!(set.is_value_type(handle), Ok(false)) {
                continue;
            }
            let name = set.full_type_name(handle).unwrap_or_default();
            match set.class_fields(handle) {
                Ok((_, placements)) => {
                    for placement in placements {
                        lines.push(format!(
                            "{}|{name}|04{:06x}|{}",
                            module.name, placement.field, placement.offset
                        ));
                    }
                }
                Err(error) => {
                    failures += 1;
                    eprintln!("  FAIL {}|{name}: {error:?}", module.name);
                }
            }
        }
    }
    lines.sort();
    for line in &lines {
        println!("{line}");
    }
    eprintln!("{} class fields placed, {failures} failures", lines.len());
    ExitCode::SUCCESS
}

fn ir(arguments: &[String]) -> ExitCode {
    let started = Instant::now();
    let set = load_set(arguments);
    let loaded = started.elapsed();
    let next = AtomicUsize::new(0);
    let totals = Mutex::new((0u64, 0u64, 0u64, 0u64, 0u64, 0u64));
    let failures: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let translator = kiln::ir::Translator::new(&set);
                let (
                    mut methods,
                    mut blocks,
                    mut values,
                    mut instructions,
                    mut conflicts,
                    mut unreachable,
                ) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
                let mut local_failures = Vec::new();
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(module) = set.modules.get(index) else {
                        break;
                    };
                    for row in 1..=module.metadata.rows(kiln::metadata::TableId::MethodDef) {
                        match translator.translate(index, row) {
                            Ok(Some(method)) => {
                                if let Err(problem) = method.validate() {
                                    let name = module
                                        .metadata
                                        .string_cell(kiln::metadata::TableId::MethodDef, row, 3)
                                        .unwrap_or("?");
                                    local_failures.push(format!(
                                        "{}  {name} (MethodDef {row}): invalid IR, {problem}",
                                        module.name
                                    ));
                                }
                                methods += 1;
                                blocks += method.blocks.len() as u64;
                                values += u64::from(method.values);
                                instructions += method
                                    .blocks
                                    .iter()
                                    .map(|b| b.instructions.len() as u64)
                                    .sum::<u64>();
                                conflicts += u64::from(method.merge_conflicts);
                                unreachable += u64::from(method.unreachable_blocks);
                            }
                            Ok(None) => {}
                            Err(error) => {
                                let name = module
                                    .metadata
                                    .string_cell(kiln::metadata::TableId::MethodDef, row, 3)
                                    .unwrap_or("?");
                                local_failures.push(format!(
                                    "{}  {name} (MethodDef {row}): {error}",
                                    module.name
                                ));
                            }
                        }
                    }
                }
                let mut total = totals.lock().expect("totals lock");
                total.0 += methods;
                total.1 += blocks;
                total.2 += values;
                total.3 += instructions;
                total.4 += conflicts;
                total.5 += unreachable;
                failures
                    .lock()
                    .expect("failures lock")
                    .extend(local_failures);
            });
        }
    });
    let (methods, blocks, values, instructions, conflicts, unreachable) =
        totals.into_inner().expect("totals lock");
    let mut failures = failures.into_inner().expect("failures lock");
    failures.sort();
    for failure in &failures {
        println!("  FAIL {failure}");
    }
    println!(
        "{} assemblies loaded in {:.2?}, {methods} methods translated into {blocks} blocks, {instructions} instructions, {values} values, {conflicts} merge type conflicts, {unreachable} unreachable blocks, {} failures, total {:.2?}",
        set.modules.len(),
        loaded,
        failures.len(),
        started.elapsed()
    );
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn disasm(arguments: &[String]) -> ExitCode {
    let [path, assembly, method] = arguments else {
        eprintln!("usage: kiln disasm <folder> <assembly> <MethodDef row>");
        return ExitCode::from(2);
    };
    let set = load_set(std::slice::from_ref(path));
    let Some(module_index) = set.find_assembly(assembly) else {
        eprintln!("{assembly} was not found");
        return ExitCode::from(1);
    };
    let Ok(row) = method.parse::<u32>() else {
        eprintln!("{method} is not a MethodDef row");
        return ExitCode::from(2);
    };
    let module = &set.modules[module_index];
    let metadata = &module.metadata;
    let table = kiln::metadata::TableId::MethodDef;
    let (Ok(rva), Ok(name), Ok(signature)) = (
        metadata.cell(table, row, 0),
        metadata.string_cell(table, row, 3),
        metadata
            .cell(table, row, 4)
            .and_then(|blob| metadata.blob(blob))
            .and_then(kiln::signature::Signature::parse),
    ) else {
        eprintln!("MethodDef {row} could not be read");
        return ExitCode::from(1);
    };
    println!("{name} {signature:?}");
    let body = module
        .image
        .tail_at_rva(rva)
        .and_then(|tail| kiln::il::MethodBody::parse(tail, rva));
    let Ok(body) = body else {
        eprintln!("the method has no IL body");
        return ExitCode::from(1);
    };
    println!(
        "max stack {}  locals {:#010x}",
        body.max_stack, body.local_signature.0
    );
    for instruction in body.instructions() {
        match instruction {
            Ok(instruction) => {
                println!(
                    "  IL_{:04x}: {} {:?}",
                    instruction.offset, instruction.opcode.name, instruction.argument
                );
            }
            Err(error) => {
                println!("  {error}");
                break;
            }
        }
    }
    ExitCode::SUCCESS
}

const CASES_PER_METHOD: u64 = 24;

struct Eligible {
    methods: std::collections::HashMap<kiln::codegen::MethodKey, kiln::codegen::CompiledMethod>,
    reasons: std::collections::BTreeMap<String, u64>,
    examined: u64,
}

fn compile_eligible(set: &kiln::loader::AssemblySet, helpers: &kiln::codegen::Helpers) -> Eligible {
    use kiln::codegen::{CompileError, MethodKey};
    use kiln::metadata::TableId;
    let compiler = kiln::codegen::Compiler::new(set, helpers);
    let mut methods = std::collections::HashMap::new();
    let mut reasons: std::collections::BTreeMap<String, u64> = Default::default();
    let mut examined = 0u64;
    for (index, module) in set.modules.iter().enumerate() {
        for row in 1..=module.metadata.rows(TableId::MethodDef) {
            let rva = module
                .metadata
                .cell(TableId::MethodDef, row, 0)
                .unwrap_or(0);
            let implementation = module
                .metadata
                .cell(TableId::MethodDef, row, 1)
                .unwrap_or(0);
            if rva == 0 || implementation & 0x3 != 0 {
                continue;
            }
            let key = MethodKey { module: index, row };
            if kiln::codegen::method_types(set, &key).is_err() {
                continue;
            }
            examined += 1;
            match compiler.compile(&key) {
                Ok(method) => {
                    methods.insert(key, method);
                }
                Err(CompileError::Unsupported(reason)) | Err(CompileError::Translate(reason)) => {
                    let reason = reason.split(" at IL").next().unwrap_or("").to_owned();
                    *reasons.entry(reason).or_default() += 1;
                }
            }
        }
    }
    let mut failed: std::collections::HashSet<MethodKey> = Default::default();
    loop {
        let wanted: Vec<MethodKey> = methods
            .values()
            .flat_map(|m| m.callees.iter())
            .filter(|c| !methods.contains_key(*c) && !failed.contains(*c))
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        if wanted.is_empty() {
            break;
        }
        for key in wanted {
            match compiler.compile(&key) {
                Ok(method) => {
                    methods.insert(key, method);
                }
                Err(_) => {
                    failed.insert(key);
                }
            }
        }
    }
    loop {
        let missing: Vec<MethodKey> = methods
            .iter()
            .filter(|(_, m)| m.callees.iter().any(|c| !methods.contains_key(c)))
            .map(|(k, _)| k.clone())
            .collect();
        if missing.is_empty() {
            break;
        }
        for key in missing {
            methods.remove(&key);
            *reasons
                .entry("calls a method that is not compiled".into())
                .or_default() += 1;
        }
    }
    Eligible {
        methods,
        reasons,
        examined,
    }
}

fn method_signature(
    set: &kiln::loader::AssemblySet,
    key: &kiln::codegen::MethodKey,
) -> Option<kiln::signature::MethodSig> {
    let metadata = &set.modules[key.module].metadata;
    let blob = metadata
        .cell(kiln::metadata::TableId::MethodDef, key.row, 4)
        .ok()?;
    match kiln::signature::Signature::parse(metadata.blob(blob).ok()?).ok()? {
        kiln::signature::Signature::Method(sig) => Some(sig),
        _ => None,
    }
}

fn jit_cases(arguments: &[String]) -> ExitCode {
    let Some((output, folders)) = arguments.split_first() else {
        return ExitCode::from(2);
    };
    let started = Instant::now();
    let set = load_set(folders);
    let helpers = kiln::jit::helpers();
    let eligible = compile_eligible(&set, &helpers);
    let mut lines = Vec::new();
    let mut testable = 0u64;
    let mut keys: Vec<&kiln::codegen::MethodKey> = eligible.methods.keys().collect();
    keys.sort_by_key(|k| (k.module, k.row));
    for key in keys {
        let Some(sig) = method_signature(&set, key) else {
            continue;
        };
        let Some(returns) = kiln::jit::ValueKind::from_sig(&sig.return_type) else {
            continue;
        };
        let Some(parameters) = sig
            .parameters
            .iter()
            .map(|p| match p {
                kiln::signature::TypeSig::IntPtr | kiln::signature::TypeSig::UIntPtr => None,
                other => kiln::jit::ValueKind::from_sig(other),
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        testable += 1;
        let module = &set.modules[key.module];
        for case in 0..CASES_PER_METHOD {
            let values: Vec<String> = parameters
                .iter()
                .enumerate()
                .map(|(position, kind)| {
                    let seed = (u64::from(key.row) << 24)
                        ^ (case << 8)
                        ^ position as u64
                        ^ (key.module as u64) << 48;
                    kiln::jit::generate_text(*kind, seed)
                })
                .collect();
            lines.push(format!(
                "{}\t{:08x}\t{case}\t{}\t{}\t{}",
                module.path.display(),
                0x0600_0000 | key.row,
                values.join(","),
                parameters
                    .iter()
                    .map(|k| k.name())
                    .collect::<Vec<_>>()
                    .join(","),
                returns.name()
            ));
        }
    }
    if let Err(error) = std::fs::write(output, lines.join("\n") + "\n") {
        eprintln!("could not write {output}: {error}");
        return ExitCode::from(1);
    }
    let mut reasons: Vec<(&String, &u64)> = eligible.reasons.iter().collect();
    reasons.sort_by(|a, b| b.1.cmp(a.1));
    for (reason, count) in reasons.iter().take(15) {
        println!("  not yet compiled: {count} {reason}");
    }
    println!(
        "{} methods with primitive signatures examined, {} compiled to x64, {testable} testable with a return value, {} cases written in {:.2?}",
        eligible.examined,
        eligible.methods.len(),
        lines.len(),
        started.elapsed()
    );
    ExitCode::SUCCESS
}

fn jit_check(arguments: &[String]) -> ExitCode {
    let Some((results, folders)) = arguments.split_first() else {
        return ExitCode::from(2);
    };
    let started = Instant::now();
    let set = load_set(folders);
    let helpers = kiln::jit::helpers();
    let eligible = compile_eligible(&set, &helpers);
    let model = kiln::objects::ObjectModel::new(&set);
    let state = kiln::runtime::install();
    if let Ok(record) = state.type_record(&model, kiln::objects::RuntimeType::String) {
        state.string_type = record;
    }
    state.empty_string(&model);
    let image = match kiln::jit::Image::link(&eligible.methods, &model) {
        Ok(image) => image,
        Err(error) => {
            eprintln!("linking failed: {error}");
            return ExitCode::from(1);
        }
    };
    let compiled_bytes: usize = eligible.methods.values().map(|m| m.code.len()).sum();
    let linked = started.elapsed();
    let modules: std::collections::HashMap<String, usize> = set
        .modules
        .iter()
        .enumerate()
        .map(|(i, m)| (m.path.display().to_string(), i))
        .collect();
    let Ok(text) = std::fs::read_to_string(results) else {
        eprintln!("could not read {results}");
        return ExitCode::from(1);
    };
    let (mut compared, mut matched, mut skipped, mut trapped) = (0u64, 0u64, 0u64, 0u64);
    let trace = std::env::var_os("KILN_TRACE").is_some();
    kiln::runtime::STRESS.store(
        std::env::var_os("KILN_GC_STRESS").is_some(),
        std::sync::atomic::Ordering::Relaxed,
    );
    let mut mismatched_methods: std::collections::BTreeMap<String, u64> = Default::default();
    let mut examples = Vec::new();
    let mut methods_seen = std::collections::HashSet::new();
    let mut object_methods = std::collections::HashSet::new();
    let mut object_cases = 0u64;
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 7 {
            continue;
        }
        let (path, token, case, values, returns, result) = (
            fields[0], fields[1], fields[2], fields[3], fields[5], fields[6],
        );
        if result.starts_with("throw") || result.starts_with("skip") {
            skipped += 1;
            continue;
        }
        let (Some(&module), Ok(token)) = (modules.get(path), u32::from_str_radix(token, 16)) else {
            skipped += 1;
            continue;
        };
        let key = kiln::codegen::MethodKey {
            module,
            row: token & 0x00FF_FFFF,
        };
        let arguments: Vec<u64> = if values.is_empty() {
            Vec::new()
        } else {
            values
                .split(',')
                .map(|v| match v {
                    "z" => 0,
                    text if text.starts_with('s') => kiln::runtime::new_string(
                        &kiln::jit::decode_units(text).unwrap_or_default(),
                    ),
                    hex => u64::from_str_radix(hex, 16).unwrap_or(0),
                })
                .collect()
        };
        let kind = kiln::jit::ValueKind::parse(returns).unwrap_or(kiln::jit::ValueKind::F64);
        if trace {
            eprintln!(
                "run {} row {} case {case} args [{values}]",
                set.modules[module].name, key.row
            );
        }
        let trapped_before = kiln::runtime::TRAPPED.load(std::sync::atomic::Ordering::Relaxed);
        let Some(raw) = image.call(&key, &arguments) else {
            skipped += 1;
            continue;
        };
        if kiln::runtime::TRAPPED.load(std::sync::atomic::Ordering::Relaxed) != trapped_before {
            trapped += 1;
            continue;
        }
        compared += 1;
        methods_seen.insert(key.clone());
        if eligible.methods.get(&key).is_some_and(|m| m.uses_objects) {
            object_methods.insert(key.clone());
            object_cases += 1;
        }
        let actual = kiln::jit::result_text(kind, raw);
        if actual == result {
            matched += 1;
        } else {
            let name = set.modules[module]
                .metadata
                .string_cell(kiln::metadata::TableId::MethodDef, key.row, 3)
                .unwrap_or("?");
            let label = format!("{}::{name} (row {})", set.modules[module].name, key.row);
            *mismatched_methods.entry(label.clone()).or_default() += 1;
            if examples.len() < 25 {
                examples.push(format!(
                    "{label} case {case} args [{values}] expected {result} got {actual}"
                ));
            }
        }
    }
    for example in &examples {
        println!("  MISMATCH {example}");
    }
    println!(
        "{} executed methods allocate objects or use fields, statics, arrays or strings, covering {object_cases} compared cases",
        object_methods.len()
    );
    println!(
        "{} methods compiled into {compiled_bytes} bytes of x64 and linked in {linked:.2?}, {} methods executed, {compared} cases compared, {matched} identical to .NET, {} mismatched in {} methods, {skipped} cases skipped (.NET threw or the case was not runnable), {trapped} cases reached a method that is not compiled yet, {} collections, total {:.2?}",
        eligible.methods.len(),
        methods_seen.len(),
        compared - matched,
        mismatched_methods.len(),
        kiln::runtime::COLLECTIONS.load(std::sync::atomic::Ordering::Relaxed),
        started.elapsed()
    );
    if compared == matched {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
