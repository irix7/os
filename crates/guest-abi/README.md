# guest-abi

The small integer ABI that the WebAssembly core uses to talk to a native shim:
one generic `host_submit`, a completion drain, capability checks, time,
entropy, console and yield/exit.

Every pointer is a 32-bit offset into guest linear memory. The host validates
it against the current memory size; no native pointer crosses the boundary.
The final ABI crate lives in the SDK as `wasm-abi`; this crate is the OS-side
import surface until that crate is tagged.
