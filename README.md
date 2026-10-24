# Kiln

Kiln compiles .NET WPF applications ahead of time into native Windows executables that run without an installed .NET runtime. It is written in Rust.

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

Console applications run today. WPF applications package and start, and stop where they first need the reflection object model (`Assembly`, `Module` and manifest resources).

## Roadmap

1. Type system: signatures, generics, type layout and member resolution across assemblies
2. IL to SSA intermediate representation
3. x64 code generation
4. Runtime: garbage collector, exception handling, type loader and interop
5. Reachability analysis that understands XAML and BAML
6. Fallback execution for code that cannot be compiled ahead of time
7. Standalone executables and MSBuild integration for `dotnet publish`
8. Reflection object model: assemblies, modules, members and manifest resources

## Building

```
cargo build --release
```
