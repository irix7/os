# demo dependency

A 120-byte ABI v12 module linked at the runtime arena base (`0x884c0000`). It
is loaded by `modules/manager` with `module_load`, entered with
`module_execute`, returns vtable status 42 and logs its entry.
