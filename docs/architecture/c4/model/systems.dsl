operator = person "User / Operator" "Invokes CortexWeave through its CLI and configures or operates an agent integration." "Person"

agentHarness = softwareSystem "Coding Agent / Agent Harness" "External system that owns reasoning-model invocation, planning, developer-tool execution, agent loops, retries, and stopping behavior." "External"

registeredWorkspace = softwareSystem "Registered Workspace" "External authoritative filesystem containing project source and documents observed and indexed by CortexWeave." "External,Authoritative"

embeddingService = softwareSystem "Embedding Service" "External OpenAI-compatible embedding provider that generates document and query vectors." "External"

cortexweave = softwareSystem "CortexWeave" "Local-first persistent context substrate that indexes registered workspaces, maintains source intelligence, agent state, structural knowledge, and verified historical Experience, and returns bounded explainable context." "CortexWeave"
