import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    TransportKind,
} from 'vscode-languageclient/node';

let client: LanguageClient | undefined;

/**
 * Find the flutmax-lsp binary.
 *
 * Search order:
 * 1. User-configured path (flutmax.lsp.path)
 * 2. Workspace-relative release build
 * 3. Workspace-relative debug build
 * 4. Extension-relative release build
 * 5. Extension-relative debug build
 * 6. System PATH (bare command name)
 */
function findLspBinary(extensionPath: string): string | undefined {
    const config = vscode.workspace.getConfiguration('flutmax');
    const configured = config.get<string>('lsp.path', '');
    if (configured && fs.existsSync(configured)) {
        return configured;
    }

    // Look relative to workspace root(s)
    const workspaceFolders = vscode.workspace.workspaceFolders ?? [];
    for (const folder of workspaceFolders) {
        const candidates = [
            path.join(folder.uri.fsPath, 'target', 'release', 'flutmax-lsp'),
            path.join(folder.uri.fsPath, 'target', 'debug', 'flutmax-lsp'),
        ];
        for (const c of candidates) {
            if (fs.existsSync(c)) {
                return c;
            }
        }
    }

    // Look relative to the extension directory (e.g. editors/vscode -> repo root)
    const repoRoot = path.resolve(extensionPath, '..', '..');
    const extRelative = [
        path.join(repoRoot, 'target', 'release', 'flutmax-lsp'),
        path.join(repoRoot, 'target', 'debug', 'flutmax-lsp'),
    ];
    for (const c of extRelative) {
        if (fs.existsSync(c)) {
            return c;
        }
    }

    // Fall back to bare command name (expects it on PATH)
    return 'flutmax-lsp';
}

export async function activate(context: vscode.ExtensionContext) {
    const config = vscode.workspace.getConfiguration('flutmax');

    if (!config.get<boolean>('lsp.enabled', true)) {
        return;
    }

    const serverPath = findLspBinary(context.extensionPath);
    if (!serverPath) {
        vscode.window.showWarningMessage(
            'flutmax-lsp binary not found. Set flutmax.lsp.path or build with `cargo build -p flutmax-lsp`.'
        );
        return;
    }

    const serverOptions: ServerOptions = {
        command: serverPath,
        transport: TransportKind.stdio,
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: 'file', language: 'flutmax' }],
    };

    client = new LanguageClient(
        'flutmax',
        'flutmax Language Server',
        serverOptions,
        clientOptions
    );

    await client.start();
}

export async function deactivate() {
    if (client) {
        await client.stop();
    }
}
