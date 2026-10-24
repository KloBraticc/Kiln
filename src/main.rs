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
