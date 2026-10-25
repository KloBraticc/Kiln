# Kiln

Kiln compiles .NET WPF applications ahead of time into native Windows executables that run without an installed .NET runtime. It is written in Rust.

> **Experimental and paused.** Nothing ships with Kiln. Voidstrap still publishes with the normal .NET SDK and the Rust `voidlauncher`, which installs the .NET runtime when it is missing. Work here continues later. The [current work](#current-work) and [TODO](#todo) sections record where it stopped and what is left.

## Goals

- Native startup and memory use on par with or better than NativeAOT.
- Works with WPF applications as they are. No changes to XAML, bindings or application code, and no AOT annotations.
- Never crashes where NativeAOT would. Code that can be proven ahead of time is compiled to native code. Code that only exists at runtime, such as reflection driven XAML loading, dynamic bindings and generated code, runs on an embedded fallback instead of failing.
- Drop in use: reference Kiln from a WPF project and publish as usual.

## Status

The assembly reader is complete:

- PE and CLI headers, including ReadyToRun images and mixed mode assemblies
- All 45 ECMA-335 metadata tables, coded indexes, and the string, user string, blob and GUID heaps
- Method bodies, exception clauses and the full IL instruction set, decoded without allocation
- A structural verifier that checks every branch target, exception clause and token

`kiln inspect <assembly or folder>...` decodes and verifies every method body it finds.

The reachability analysis understands WPF:

- Reads embedded `.g.resources` files and decodes every BAML record, including WPF's built in known types, which are read from PresentationFramework itself
- Follows calls, constructions, fields, generic instantiations, interfaces, virtual overrides, custom attributes and module initializers from the entry point, and into the BAML of every library it reaches
- Keeps types used through reflection: everything named by BAML, `typeof` and type name strings passed to reflection APIs, and types activated through generic methods such as dependency injection registration
- Reports the methods whose reflection cannot be resolved ahead of time, which are left to the fallback

`kiln reach <application> <folder>...` prints the result, `kiln reach-methods` lists every reachable method and `kiln baml` lists the types each BAML document creates.

The fallback runs code that is not compiled ahead of time:

- An IL interpreter that shares the heap, type records, statics, string literals and garbage collector with compiled code
- Exception handling with catch, filter, finally and fault clauses, virtual and interface dispatch, delegates, boxing and static constructors
- Struct values in locals, arguments, fields, statics, arrays and boxes, with exact CoreCLR layouts and garbage collector reference maps for embedded references
- Generic classes, structs, interfaces and methods, with exact layouts, statics, static constructors and virtual tables per instantiation, including static abstract interface members
- Platform invoke into any Windows library, with primitives, pointers, byrefs, `bool`, Unicode, ANSI and UTF-8 strings, blittable arrays, structs by value, `SafeHandle`, `HandleRef`, delegates, the common `MarshalAs` forms, `SetLastError`, and native versions of the runtime QCalls ordinary code reaches
- Native code calls back into managed code through generated entry stubs, for `UnmanagedCallersOnly` function pointers and for delegates passed to native code
- Globalization through the Windows NLS APIs, so culture aware comparison, casing, number and date formatting work without ICU
- GC handles, dependent handles, locks and identity hash codes, so `ConditionalWeakTable`, `GCHandle` and `lock` work as they do in .NET
- Native versions of runtime intrinsics, such as math through the Windows C runtime, interlocked operations and string resources read from each assembly
- Compiled code and the fallback call each other freely: calls to methods that are not compiled go through generated stubs, and a fault or exception unwinds compiled frames back to the fallback
- Compiled code checks array bounds, casts, overflow and null receivers, and turns hardware faults into the matching .NET exceptions

`kiln fallback-report <application> <folder>...` shows how much of an application the fallback can load and what still blocks the rest.

Applications run without an installed .NET runtime:

- `kiln run <application> <folder>... [-- arguments]` runs an application's entry point on Kiln, passing its `runtimeconfig.json` properties to `AppContext`
- `kiln publish <application> <folder>... -o <output.exe>` compiles the reachable methods to x64 ahead of time and writes one Windows executable holding the Kiln runtime, the compiled code and the assemblies the application uses, next to the native libraries it imports
- `msbuild/Kiln.targets` runs `kiln publish` after `dotnet publish -r win-x64 --self-contained -p:PublishKiln=true` and writes the executable to the `native` folder of the publish directory

The fallback also runs threads and the thread pool (a global interpreter lock released around native calls, waits and every 16k interpreted instructions), monitors, type initialization gates across threads, multi dimensional arrays, `UnsafeAccessor`, COM wrappers in both directions, COM class activation and most of the reflection object model.

Console applications run today. Voidstrap packages into a 203 MB executable (120,121 of 269,896 reachable methods compiled to x64, the rest on the fallback) and opens its launch menu, the About window and the JumpList without an installed .NET runtime. Reaching the menu takes about 50 seconds, against well under a second on .NET.

Diagnostics are switched on with environment variables:

| Variable | Effect |
| --- | --- |
| `KILN_LOG=<file>` | Writes unsupported features, throws, COM refusals and faults with the managed stack |
| `KILN_TRACE=1` | Prints a fault report with a native backtrace |
| `KILN_CALLS=1` | Prints every managed and native call, indented by depth and prefixed with the thread id |
| `KILN_TIMING=1` | Prints phase timings and counters on exit |
| `KILN_PROFILE=1` | Samples the main thread every millisecond and prints the hottest functions |
| `KILN_COMPILE_REPORT=1` | Prints why each method was not compiled ahead of time |
| `KILN_GC_STRESS=1` | Collects garbage on every allocation |

## Current work

When work paused, these were in progress:

- The launch menu's minimize button. The 10x1 `Rectangle` glyph in the Wpf.Ui title bar does not draw in the layered (`AllowsTransparency`) menu window, but it does draw in normal windows such as About. The button, its hover state and its tooltip all work. Isolated probes of the same rectangle, the real Wpf.Ui `TitleBar`, BAML loading and layered windows all match .NET. The cause sits somewhere between the app's window setup and the layered presentation path.
- Hover feedback reported as dead across the UI. Probes show that `IsMouseOver`, `MouseEnter`, triggers, animations and repainting of a Wpf.Ui button in a layered `WindowChrome` window all behave as on .NET. Check again in the app after the attribute usage fix below, since broken `ElementName` bindings and name scopes were a likely cause.
- After the attribute usage fix the app reached the menu in 53 seconds, then exited with code 1. It logged nothing and threw no managed exception. Trace it with `KILN_CALLS=1` and search for `Shutdown`, `Environment::Exit`, `ShowDialog` and the dialog's close path.
- "Not responding": the window stops answering while it opens another window (About, Settings), because that work runs interpreted on the UI thread.

Fixed in the last round, each checked against .NET with a probe:

- Boxed value types dispatched through virtual tables passed the box as `this` instead of its data, so `object.Equals(a, b)` was false for boxed `char` and structs. This broke WPF triggers, for example the title bar showed its Help and Maximize buttons.
- Class initializers of precise (not `beforefieldinit`) types now run before their platform invokes. GDI+ is started in such an initializer, so every `System.Drawing` call failed with "GDI+ is not properly initialized".
- `Type.GetTypeFromCLSID`, `Activator.CreateInstance` of COM classes through `CoCreateInstance`, and `Type.GUID` of those types, which the WPF JumpList needs.
- `ParseAttributeUsageAttribute` returned `Inherited` and `AllowMultiple` in swapped order, so inherited custom attributes were never found. WPF finds name scopes through the inherited `[NameScopeProperty]`, so every `x:Name`, `FindName` and `ElementName` binding in compiled XAML failed.

## TODO

### Stability

- [ ] Find why Voidstrap exits with code 1 right after "Opening menu" since the attribute usage fix
- [ ] Draw the minimize glyph in the layered launch menu window
- [ ] Confirm hover, pressed and focus visuals across every Wpf.Ui control in the real app, not only in probes
- [ ] UI Automation. `UiaCoreProviderApi.RawUiaHostProviderFromHwnd` once crashed reading address 0x10 when an automation client queried the window. Screen readers and Narrator will hit this. Later runs logged refused `QueryInterface` calls on `IRawElementProviderSimple` instead, so CCWs for automation providers need every interface UIA asks for
- [ ] WIC `IStream` CCW `Write` crashes when a PNG is saved through `PngBitmapEncoder`
- [ ] The taskbar icon is generic while the window icon shows. Check the `CreateIconIndirect` path and `WM_SETICON`
- [ ] Hundreds of delegate `BeginInvoke` and `EndInvoke` methods are logged as unsupported internal calls during scanning. Stop reporting methods nobody calls, or implement them as .NET does (`PlatformNotSupportedException`)
- [ ] Every `Reflect` operation not routed in the dispatcher reports "metadata scope from outside the fallback". Make unrouted operations report their own name instead of falling into `metadata_call`
- [ ] Cancellation paths in the app (the Roblox account check, downloads) under the fallback
- [ ] Run Voidstrap's Settings window (the main window) and every page, the Roblox web window (WebView2), and a real Roblox launch
- [ ] Rerun every probe after each change (`bx`, `md`, `rf`, `ua`, `ptr`, `ic`, `font`, `rr`, `bm`, `co`, `tb`, `hv`, `si`), keep them as a regression suite, and add them to the repository with their expected .NET output
- [ ] A crash dump writer for faults outside compiled code, with the managed stack, the last native call and the last instruction
- [ ] Garbage collector stress runs (`KILN_GC_STRESS`) over the whole Voidstrap startup
- [ ] A thread safety audit of every `RefCell` in the interpreter, now that the GIL is released around native calls, waits and every 16k instructions
- [ ] Finalizers and `SafeHandle` release on shutdown
- [ ] `AppDomain.ProcessExit`, `AppContext` switches and unhandled exception events behave as on .NET
- [ ] `Environment.Exit` from any thread
- [ ] Stack overflow detection with a clean report instead of an access violation
- [ ] Out of memory handling in the allocator and the garbage collector

### Diagnostics

- [ ] Record every failed native HRESULT and `GetLastError` value in `KILN_LOG`, with the call that produced it
- [ ] Warn when managed code calls into `MethodTable`, `TypeHandle` or `MethodTableAuxiliaryData` members, since those read CoreCLR layouts that Kiln imitates
- [ ] An exit summary: run time, interpreted and compiled call counts, fallback entries, unsupported features hit, and first chance exceptions by type
- [ ] Show hidden errors the way the app does: every first chance exception with its stack, every exception swallowed by an empty `catch`, every binding error
- [ ] A `kiln doctor` command that runs an application, collects the log, and prints the first unsupported feature, the first fault and the slowest methods
- [ ] Symbolized native stacks for the Kiln runtime itself (DWARF from the GNU toolchain, or a switch to MSVC and PDBs)
- [ ] Probes that compare against .NET automatically and print only the differences

### Speed (goal: start faster than NativeAOT)

- [ ] Reaching the menu takes about 50 seconds. Profile with `KILN_PROFILE=1` and `KILN_TIMING=1` and remove the top costs one by one
- [ ] Compile generic instantiations ahead of time (shared canonical code plus exact instantiations for value types)
- [ ] Compile methods that use structs by value, exception handling, thread statics and `calli`
- [ ] Compile methods only reached through BAML, reflection and delegates when the target is known ahead of time
- [ ] Precompute type layouts, virtual tables, interface maps and static bases at publish time and store them in the executable
- [ ] Parse BAML and resource dictionaries at publish time
- [ ] Freeze objects created by static constructors without side effects (strings, arrays, lookup tables) into the image
- [ ] Replace per call `HashMap` lookups in the interpreter with indexed tables, and add inline caches for calls, fields and casts
- [ ] Decode IL once into a compact threaded form instead of decoding each instruction every time it runs
- [ ] A faster hasher (`FxHash` or `ahash`) for every interpreter map
- [ ] Tiered execution: interpret first, then compile hot methods at runtime with the existing code generator
- [ ] A real register allocator and SSA optimizations (inlining, constant folding, bounds check removal, devirtualization)
- [ ] Vector intrinsics (`Vector128`, SSE2, AVX2) in compiled code and the fallback, so span and string helpers stop running scalar loops
- [ ] A generational collector, or at least a faster allocator, with card marking for compiled code
- [ ] Show the first window before loading assemblies it does not need
- [ ] Measure memory use against .NET and NativeAOT, and keep it lower
- [ ] Keep the UI thread responsive: never hold the GIL across long native calls, and yield more often in long interpreted loops

### Coverage

- [ ] `Reflection.Emit`, `DynamicMethod` and `Expression.Compile` (run expression trees through the interpreter instead)
- [ ] `System.Text.Json`, both source generated and reflection based, over the whole Voidstrap settings model
- [ ] `HttpClient`, `SslStream` and sockets under load, including cancellation and the thread pool's I/O completion port
- [ ] WebView2, its COM interfaces and its callbacks
- [ ] Windows Runtime projections from CsWinRT
- [ ] Full RCW and CCW support for every COM interface that WPF, WIC, DirectWrite, UIA, TSF and the shell use, including aggregation and `IDispatch`
- [ ] `Type.GetTypeFromProgID` and the other COM lookup helpers
- [ ] Multi dimensional arrays with non zero lower bounds
- [ ] `TypedReference`, `__arglist` and varargs
- [ ] Function pointers to generic methods, and `ldvirtftn` on generic virtual methods
- [ ] Every remaining runtime QCall and FCall that `kiln fallback-report` lists for Voidstrap
- [ ] `EventSource` and `NativeRuntimeEventSource` as no ops that never throw
- [ ] Culture data without ICU for every culture Voidstrap ships translations for, including right to left languages and Thai, Chinese and Japanese text
- [ ] Satellite assemblies and manifest resources read from the packaged executable

### Packaging

- [ ] Shrink the executable: drop unreached assemblies and methods, compress the packaged assemblies, deduplicate string literals
- [ ] Sign the executable, and check it against Windows Defender and SmartScreen before any release
- [ ] Hook `kiln publish` into `publish-all.ps1` and the Windows release workflow behind an opt in switch. Only remove `voidlauncher` once Kiln builds pass every check above
- [ ] An upgrade path that copies the Kiln runtime files together with the executable (the in app upgrade copies only the exe today)
- [ ] Crash reports from users with enough detail to reproduce without a debugger

### Testing

- [ ] Run the .NET runtime's own IL test suites (`src/tests/JIT`, `src/tests/Loader`) on Kiln
- [ ] Run WPF's own tests and the Wpf.Ui gallery
- [ ] Run the probe suite in CI on every change
- [ ] Fuzz the metadata reader and the IL decoder

## Roadmap

1. Type system: signatures, generics, type layout and member resolution across assemblies
2. IL to SSA intermediate representation
3. x64 code generation
4. Runtime: garbage collector, exception handling, type loader and interop
5. Reachability analysis that understands XAML and BAML
6. Fallback execution for code that cannot be compiled ahead of time
7. Standalone executables and MSBuild integration for `dotnet publish`
8. Reflection object model: assemblies, modules, members and manifest resources
9. Running real WPF applications end to end (Voidstrap as the target)
10. Startup speed beyond NativeAOT
11. Opt in use in Voidstrap's Windows release, replacing `voidlauncher`

## Building

```
cargo build --release
```
