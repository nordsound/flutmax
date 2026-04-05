import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import * as os from 'os';

/**
 * Wait for diagnostics to stabilise on a URI.
 *
 * The function polls `vscode.languages.getDiagnostics` and returns once
 * the list has been unchanged for `stableMs` milliseconds, or after
 * `timeoutMs` has elapsed (whichever comes first).
 */
export async function waitForStableDiagnostics(
    uri: vscode.Uri,
    stableMs: number = 2000,
    timeoutMs: number = 15000
): Promise<vscode.Diagnostic[]> {
    return new Promise<vscode.Diagnostic[]>((resolve) => {
        let lastDiagnostics: string = '';
        let stableTimer: ReturnType<typeof setTimeout> | undefined;
        const startTime = Date.now();

        const check = () => {
            const diags = vscode.languages.getDiagnostics(uri);
            const serialised = JSON.stringify(diags.map(d => ({
                msg: d.message,
                sev: d.severity,
                range: d.range,
            })));

            if (serialised !== lastDiagnostics) {
                lastDiagnostics = serialised;
                if (stableTimer) { clearTimeout(stableTimer); }
                stableTimer = setTimeout(() => {
                    if (interval) { clearInterval(interval); }
                    resolve(vscode.languages.getDiagnostics(uri));
                }, stableMs);
            }

            if (Date.now() - startTime > timeoutMs) {
                if (stableTimer) { clearTimeout(stableTimer); }
                if (interval) { clearInterval(interval); }
                resolve(vscode.languages.getDiagnostics(uri));
            }
        };

        const interval = setInterval(check, 200);
        // Fire immediately once as well
        check();
    });
}

/**
 * Open a temporary `.flutmax` file with the given source text in VS Code,
 * returning the document and a cleanup function.
 */
export async function openFlutmaxSource(source: string): Promise<{
    doc: vscode.TextDocument;
    uri: vscode.Uri;
    cleanup: () => Promise<void>;
}> {
    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'flutmax-test-'));
    const tmpFile = path.join(tmpDir, 'test.flutmax');
    fs.writeFileSync(tmpFile, source, 'utf-8');

    const uri = vscode.Uri.file(tmpFile);
    const doc = await vscode.workspace.openTextDocument(uri);

    // Ensure the language id is flutmax
    if (doc.languageId !== 'flutmax') {
        await vscode.languages.setTextDocumentLanguage(doc, 'flutmax');
    }
    await vscode.window.showTextDocument(doc);

    const cleanup = async () => {
        await vscode.commands.executeCommand('workbench.action.closeActiveEditor');
        try { fs.unlinkSync(tmpFile); } catch { /* ignore */ }
        try { fs.rmdirSync(tmpDir); } catch { /* ignore */ }
    };

    return { doc, uri, cleanup };
}

/**
 * Get diagnostics for a source string using the temp-file pattern.
 */
export async function getDiagnosticsForSource(
    source: string,
    stableMs: number = 2000,
    timeoutMs: number = 15000
): Promise<vscode.Diagnostic[]> {
    const { uri, cleanup } = await openFlutmaxSource(source);
    try {
        return await waitForStableDiagnostics(uri, stableMs, timeoutMs);
    } finally {
        await cleanup();
    }
}
