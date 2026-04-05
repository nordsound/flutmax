import * as assert from 'assert';
import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import * as os from 'os';
import {
    getDiagnosticsForSource,
    openFlutmaxSource,
    waitForStableDiagnostics,
} from './helpers';

/**
 * Check whether the LSP server binary is available.
 * If not, all LSP-dependent tests are skipped gracefully.
 */
function lspBinaryAvailable(): boolean {
    const config = vscode.workspace.getConfiguration('flutmax');
    const explicit = config.get<string>('lsp.path', '');
    if (explicit && fs.existsSync(explicit)) { return true; }

    // Check common build locations relative to repo root.
    // The extension lives at <repo>/editors/vscode, so repo root is ../..
    const extPath = vscode.extensions.all.find(
        e => e.packageJSON?.name === 'flutmax'
    )?.extensionPath;
    if (extPath) {
        const repoRoot = path.resolve(extPath, '..', '..');
        if (fs.existsSync(path.join(repoRoot, 'target', 'release', 'flutmax-lsp'))) { return true; }
        if (fs.existsSync(path.join(repoRoot, 'target', 'debug', 'flutmax-lsp'))) { return true; }
    }

    return false;
}

suite('Diagnostics', function () {
    suiteSetup(function () {
        if (!lspBinaryAvailable()) {
            console.log('[test] flutmax-lsp binary not found, skipping diagnostics tests');
            this.skip();
        }
    });

    test('should report no errors for valid source', async function () {
        this.timeout(20000);
        const source = [
            'in 0 (freq): float;',
            'out 0 (audio): signal;',
            'wire osc = cycle~(freq);',
            'out[0] = osc;',
        ].join('\n') + '\n';

        const diagnostics = await getDiagnosticsForSource(source);
        const errors = diagnostics.filter(
            d => d.severity === vscode.DiagnosticSeverity.Error
        );
        assert.strictEqual(
            errors.length,
            0,
            `unexpected errors: ${errors.map(d => d.message).join(', ')}`
        );
    });

    test('should report parse error for invalid syntax', async function () {
        this.timeout(20000);
        const source = 'wire osc = cycle~(;\n';

        const diagnostics = await getDiagnosticsForSource(source);
        const errors = diagnostics.filter(
            d => d.severity === vscode.DiagnosticSeverity.Error
        );
        assert.ok(
            errors.length > 0,
            'should report at least one parse error'
        );
    });

    test('should report multiple errors with recovery', async function () {
        this.timeout(20000);
        const source = 'wire a = ;\nwire b = ;\nwire c = cycle~(440);\n';

        const diagnostics = await getDiagnosticsForSource(source);
        const errors = diagnostics.filter(
            d => d.severity === vscode.DiagnosticSeverity.Error
        );
        assert.ok(
            errors.length >= 2,
            `should report at least 2 errors, got ${errors.length}`
        );
    });

    test('should recover from missing semicolon and report subsequent errors', async function () {
        this.timeout(20000);
        // Line 1: missing semicolon after mul~(...)
        // Line 2: undefined_wire reference (separate error)
        // Line 3: valid
        const source = [
            'out audio: signal;',
            'wire osc = cycle~(440);',
            'wire broken = mul~(osc, 0.5)',   // <-- missing semicolon
            'wire gain = mul~(undefined_wire, 0.3);',  // <-- separate error
            'out[0] = osc;',
        ].join('\n') + '\n';

        const diagnostics = await getDiagnosticsForSource(source);
        const errors = diagnostics.filter(
            d => d.severity === vscode.DiagnosticSeverity.Error
        );

        // Should report at least 2 errors: missing semicolon AND the subsequent statement
        assert.ok(
            errors.length >= 2,
            `should report at least 2 errors (missing semicolon + downstream), got ${errors.length}: ${errors.map(d => `L${d.range.start.line + 1}: ${d.message}`).join('; ')}`
        );
    });

    test('should report errors on multiple lines without semicolons', async function () {
        this.timeout(20000);
        const source = [
            'wire a = cycle~(440)',    // missing ;
            'wire b = noise~()',       // missing ;
            'wire c = mul~(a, 0.5);',  // valid
        ].join('\n') + '\n';

        const diagnostics = await getDiagnosticsForSource(source);
        const errors = diagnostics.filter(
            d => d.severity === vscode.DiagnosticSeverity.Error
        );

        assert.ok(
            errors.length >= 2,
            `should report at least 2 errors for two missing semicolons, got ${errors.length}: ${errors.map(d => `L${d.range.start.line + 1}: ${d.message}`).join('; ')}`
        );
    });

    test('should clear diagnostics when source is fixed', async function () {
        this.timeout(30000);

        // 1. Open broken source
        const brokenSource = 'wire osc = ;\n';
        const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'flutmax-test-'));
        const tmpFile = path.join(tmpDir, 'test.flutmax');
        fs.writeFileSync(tmpFile, brokenSource, 'utf-8');

        const uri = vscode.Uri.file(tmpFile);
        const doc = await vscode.workspace.openTextDocument(uri);
        if (doc.languageId !== 'flutmax') {
            await vscode.languages.setTextDocumentLanguage(doc, 'flutmax');
        }
        await vscode.window.showTextDocument(doc);

        // 2. Wait for errors
        const errorDiags = await waitForStableDiagnostics(uri, 2000, 15000);
        const errors = errorDiags.filter(
            d => d.severity === vscode.DiagnosticSeverity.Error
        );
        assert.ok(errors.length > 0, 'should have errors for broken source');

        // 3. Fix the source by replacing the entire content
        const fixedSource = 'wire osc = cycle~(440);\n';
        const editor = vscode.window.activeTextEditor;
        if (editor) {
            const fullRange = new vscode.Range(
                doc.positionAt(0),
                doc.positionAt(doc.getText().length)
            );
            await editor.edit(editBuilder => {
                editBuilder.replace(fullRange, fixedSource);
            });
        }

        // 4. Wait for diagnostics to clear
        const clearedDiags = await waitForStableDiagnostics(uri, 2000, 15000);
        const remainingErrors = clearedDiags.filter(
            d => d.severity === vscode.DiagnosticSeverity.Error
        );
        assert.strictEqual(
            remainingErrors.length,
            0,
            `errors should clear after fix, but got: ${remainingErrors.map(d => d.message).join(', ')}`
        );

        // Cleanup
        await vscode.commands.executeCommand('workbench.action.closeActiveEditor');
        try { fs.unlinkSync(tmpFile); } catch { /* ignore */ }
        try { fs.rmdirSync(tmpDir); } catch { /* ignore */ }
    });
});
