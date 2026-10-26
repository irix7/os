# runtime manager

First downloadable manager slice. It reads `manager.dep=<url>,<sha256>`, calls
the ABI v12 `module_load` for region 0 and `module_execute` with a
`ModuleContext`, then publishes the result in the fixed `ModuleStatus` block
read by MCP `GET /modules`.

This is not the full #217 manager: resolution, manifest v2, signatures,
package-index fetching and multiple regions are later slices. The module is
linked at slot B (`0x88480000`); the dependency arena starts at
`0x884c0000`.
