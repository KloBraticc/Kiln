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
