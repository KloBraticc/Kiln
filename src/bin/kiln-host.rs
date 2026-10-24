use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(run)
        .map(|worker| worker.join().unwrap_or(101))
        .map_or(ExitCode::from(101), |code| ExitCode::from(code as u8))
}

fn run() -> i32 {
    let Ok(executable) = std::env::current_exe() else {
        eprintln!("Kiln cannot find its own executable");
        return 134;
    };
    let bytes: &'static [u8] = match std::fs::read(&executable) {
        Ok(bytes) => Box::leak(bytes.into_boxed_slice()),
        Err(error) => {
            eprintln!("{}: {error}", executable.display());
            return 134;
        }
    };
    let helpers = kiln::jit::helpers();
    let (payload, slices) = match kiln::package::decode(bytes, &helpers) {
        Ok(Some(decoded)) => decoded,
        Ok(None) => {
            eprintln!("this executable has no packaged application");
            return 134;
        }
        Err(error) => {
            eprintln!("the packaged application is damaged: {error}");
            return 134;
        }
    };
    let directory = executable.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut set = kiln::loader::AssemblySet::default();
    for ((name, _), slice) in payload.modules.iter().zip(slices) {
        if let Err(error) = set.add_bytes(&directory.join(name), slice) {
            eprintln!("{name}: {error}");
            return 134;
        }
    }
    let mut properties = kiln::host::default_properties(&format!("{}\\", directory.display()));
    properties.extend(payload.properties.iter().cloned());
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let methods = payload.methods;
    kiln::host::launch(&set, payload.application, &properties, &arguments, move |_| methods)
}
