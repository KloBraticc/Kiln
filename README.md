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

## Roadmap

1. Type system: signatures, generics, type layout and member resolution across assemblies
2. IL to SSA intermediate representation
3. x64 code generation
4. Runtime: garbage collector, exception handling, type loader and interop
5. Reachability analysis that understands XAML and BAML
6. Fallback execution for code that cannot be compiled ahead of time
7. MSBuild integration for `dotnet publish`

## Building

```
cargo build --release
```
