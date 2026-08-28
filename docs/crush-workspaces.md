# Using CortexWeave with More Than One Crush Project

This guide has two separate jobs:

1. Register a project with CortexWeave once.
2. Tell Crush which registered project to use whenever it starts there.

Registration is not the same as Crush setup. Registering adds a project to
CortexWeave's known-workspace list. The Crush setting picks the right project
for the Crush session you are currently running.

## Before You Start

Use the CortexWeave release binary built from this repository. The examples
below assume CortexWeave lives at `C:\dev\CortexWeave`.

Choose a short, unique name for each project. For example, the project at
`C:\Users\Capta\dev.work\projects\agentic.things\OPiHype` can be named
`opihype`.

## Step 1: Register the Project Once

Open PowerShell and run this command once for the project:

```powershell
& "C:\dev\CortexWeave\target\release\cortexweave.exe" `
  --config "C:\dev\CortexWeave\cortexweave.toml" `
  workspace add "C:\Users\Capta\dev.work\projects\agentic.things\OPiHype" `
  --name opihype
```

Replace the project path and name for each additional project. CortexWeave will
print a project ID when registration succeeds. Keep the short name handy; it is
usually easier to read than the ID.

To see every registered project, run:

```powershell
& "C:\dev\CortexWeave\target\release\cortexweave.exe" `
  --config "C:\dev\CortexWeave\cortexweave.toml" `
  workspace list
```

You do not repeat this step each time you use Crush. Adding the same path again
is safe, but unnecessary.

## Step 2: Add the Project-Local Crush Setting

In the root folder of the project, create or edit a file named `.crushrc`.
This is a text file, not a PowerShell command. Start with the reusable
[`.crushrc.example`](../.crushrc.example) template from the CortexWeave
repository, or add this entry directly:

```text
mcp add cortexweave \
  --type stdio \
  --command C:/dev/CortexWeave/target/release/cortexweave.exe \
  --args --config \
  --args C:/dev/CortexWeave/cortexweave.toml \
  --args serve \
  --args --workspace-root \
  --args "$PWD" \
  --timeout 120
```

For OPiHype, the file is:

```text
C:\Users\Capta\dev.work\projects\agentic.things\OPiHype\.crushrc
```

`$PWD` means “the folder where Crush was started.” It is passed directly to
CortexWeave as the `--workspace-root` argument, so CortexWeave selects the
registered OPiHype workspace. Starting Crush from a subfolder also works.

`--timeout 120` gives the local server up to two minutes to start. Keep it in
the project configuration so a busy machine or a first-time workspace scan does
not make Crush abandon startup too early.

## Daily Use

Open Crush from the project folder as usual. You do not need to supply a
workspace ID or a command each time. CortexWeave starts watching registered
projects after Crush connects and reconciles changes in the background.

## Adding Another Project

Repeat Step 1 for the new project's folder and name. Then put the same Step 2
entry in that new project's `.crushrc`. The entry is identical because `$PWD`
resolves to whichever project Crush is running in.

## When the Automatic Selection Is Not Used

The project-local `.crushrc` is the straightforward option. Without it, or when
one Crush configuration serves several projects, CortexWeave tools need an
explicit `workspace` value. Use the short name, such as `opihype`, an absolute
project path, or the project ID shown by `workspace list`.

Do not put `--args "$PWD"` in a global Crush configuration: there, `$PWD` can
point at Crush's configuration folder instead of your project.
