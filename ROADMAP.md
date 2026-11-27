# Roadmap

The RV64 prototype boots on QEMU, runs isolated programs and enforces
goal-scoped file access and byte budgets. A pinned mock planner can delegate
a native tool call. The filesystem can stage and publish one reviewed rename.
[Run these examples in QEMU](docs/qemu-walkthroughs.md)

Section 23 of the architecture document defines the implementation phases
and the evidence required for each native release. The kernel's `0.0.x`
package version identifies the build.
[Development roadmap](https://github.com/clavenar/specifications/blob/main/architecture/architecture-and-vision.md#23-development-roadmap)

Section 24 defines the first agentic MVP and records the evidence for each
file-organizer scenario, including which cases still require the host broker.
[MVP demonstration and status](https://github.com/clavenar/specifications/blob/main/architecture/architecture-and-vision.md#241-demonstration-scenario)
