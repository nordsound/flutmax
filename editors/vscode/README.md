# flutmax VS Code Extension

VS Code support for the flutmax language (`.flutmax` -> `.maxpat` transpiler).

## Features

- **Syntax highlighting**: Syntax coloring via TextMate grammar
  - Keywords (`wire`, `in`, `out`)
  - Type names (`signal`, `float`, `int`, `bang`, `list`, `symbol`)
  - Tilde-suffixed objects (`cycle~`, `mul~`, `biquad~`)
  - Comments, numbers, strings
- **Language configuration**: Comment toggle (Cmd+/), bracket pairs, auto-closing brackets
- **Word selection**: Double-click to select entire `cycle~`
- **Snippets**: Input helpers for common patterns

## Installation

### Install from .vsix

```bash
cd editors/vscode
npx @vscode/vsce package
code --install-extension flutmax-0.1.0.vsix
```

### Development mode

1. Open `editors/vscode/` in VS Code
2. Press F5 to launch the Extension Development Host
3. Open a `.flutmax` file

## Snippets

| prefix | Description |
|--------|-------------|
| `in` | Input port declaration |
| `out` | Output port declaration |
| `wire` | Wire declaration |
| `wire~` | Wire declaration with signal object |
| `outa` | Output assignment |
| `synth` | Simple synth template |
| `filter` | Filter template |
| `stereo` | Stereo output template |
