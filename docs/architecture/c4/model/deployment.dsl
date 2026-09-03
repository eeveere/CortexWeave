deploymentEnvironment "Local development" {
    deploymentNode "Developer Workstation" "A developer machine running local-first CortexWeave." "Windows/macOS/Linux" {
        deploymentNode "CortexWeave Rust Process" "One process launched for CLI operation or MCP stdio serving." "Rust / Tokio" {
            containerInstance cortexweave.application
        }
        deploymentNode "SQLite Database File" "Local CortexWeave database file." "SQLite" {
            containerInstance cortexweave.database
        }
        infrastructureNode "Registered Workspace Filesystem" "Authoritative project source/documents under registered roots." "Filesystem"
        infrastructureNode "Agent Harness / MCP Client Process" "Distinct process; may run on the same workstation." "External process"
        infrastructureNode "Embedding Server Process" "OpenAI-compatible provider; may run locally but remains external to CortexWeave." "HTTP service"
    }
}
