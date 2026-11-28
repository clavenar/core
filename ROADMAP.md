# Roadmap

Section 23 of the architecture document defines the native release gates.
RV64 on QEMU `virt` is the reference platform. The planned AMD64 port targets
KVM Goal Machines. The kernel package version identifies an individual build.
[Development roadmap](https://github.com/clavenar/specifications/blob/main/architecture/architecture-and-vision.md#23-development-roadmap)

| Roadmap area | Evidence in the RV64 prototype |
| --- | --- |
| Agentic MVP, phases 9–10 | A worker stages up to two renames, a separate supervisor reviews the batch, and a monitor observes publication. A pinned goal can also call Ollama, run MCP over guest stdio and invoke a read-only MCP tool through the host broker. [Reviewed publication](docs/qemu-walkthroughs.md#observe-a-reviewed-publication) · [Combined model and MCP run](docs/qemu-walkthroughs.md#run-ollama-and-both-mcp-searches-in-one-goal) |
| Replay and recovery, phases 11–14 | A recorded model and tool decision can be replayed offline. A read-denial watch has a bounded response, a durable rename journal restores committed files, and checkpointed searches can recover after reboot. [Replay](docs/qemu-walkthroughs.md#replay-a-combined-goal-offline) · [Read-denial watch](docs/qemu-walkthroughs.md#watch-for-a-read-scope-denial) · [Durable batch](docs/qemu-walkthroughs.md#persist-two-renames-in-one-commit) · [Checkpoint recovery](docs/qemu-walkthroughs.md#recover-a-host-mcp-search-checkpoint) |
| Standalone networking, phase 15 | `netd` handles Ethernet, IPv4, UDP and TCP. A goal grant admits one native HTTPS GET. `netd` checks the DNS answer against the permitted address and authenticates the original hostname. Remote MCP and destination policy for additional endpoints remain release requirements. [HTTP probe](docs/qemu-walkthroughs.md#fetch-an-http-response-over-native-tcp) · [Goal-authorized HTTPS](docs/qemu-walkthroughs.md#fetch-an-https-response-over-native-tcp) |

Section 24 maps the file-organizer scenarios to their QEMU walkthroughs and
records the scope of each demonstration. [MVP scenario status](https://github.com/clavenar/specifications/blob/main/architecture/architecture-and-vision.md#241-demonstration-scenario)
