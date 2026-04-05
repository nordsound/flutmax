import * as assert from 'assert';
import * as vscode from 'vscode';
import * as fs from 'fs';
import * as path from 'path';
import { openFlutmaxSource } from './helpers';

/**
 * Check whether the LSP server binary is available.
 */
function lspBinaryAvailable(): boolean {
    const config = vscode.workspace.getConfiguration('flutmax');
    const explicit = config.get<string>('lsp.path', '');
    if (explicit && fs.existsSync(explicit)) { return true; }

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

/**
 * Helper: extract plain string labels from a CompletionList.
 */
function extractLabels(completions: vscode.CompletionList): string[] {
    return completions.items.map(item =>
        typeof item.label === 'string' ? item.label : item.label.label
    );
}

/**
 * Small delay to give the LSP time to process the document before
 * requesting completions.
 */
function delay(ms: number): Promise<void> {
    return new Promise(resolve => setTimeout(resolve, ms));
}

suite('Completion', function () {
    suiteSetup(function () {
        if (!lspBinaryAvailable()) {
            console.log('[test] flutmax-lsp binary not found, skipping completion tests');
            this.skip();
        }
    });

    test('should provide keyword completions', async function () {
        this.timeout(15000);
        const source = '\n';
        const { doc, cleanup } = await openFlutmaxSource(source);
        try {
            await delay(2000); // wait for LSP to initialise
            const position = new vscode.Position(0, 0);
            const completions = await vscode.commands.executeCommand<vscode.CompletionList>(
                'vscode.executeCompletionItemProvider', doc.uri, position
            );
            assert.ok(completions, 'should return completions');
            const labels = extractLabels(completions);
            assert.ok(labels.includes('wire'), 'should include wire keyword');
            assert.ok(labels.includes('in'), 'should include in keyword');
            assert.ok(labels.includes('out'), 'should include out keyword');
        } finally {
            await cleanup();
        }
    });

    test('should provide Max object completions when objdb is loaded', async function () {
        this.timeout(15000);
        const source = 'wire osc = \n';
        const { doc, cleanup } = await openFlutmaxSource(source);
        try {
            await delay(2000);
            const position = new vscode.Position(0, 11);
            const completions = await vscode.commands.executeCommand<vscode.CompletionList>(
                'vscode.executeCompletionItemProvider', doc.uri, position
            );
            assert.ok(completions, 'should return completions');
            const labels = extractLabels(completions);
            // When the object database is loaded, we expect many completions
            // (keywords + Max objects). If objdb is not available, at minimum
            // the keywords should still be present.
            assert.ok(
                labels.length >= 10,
                `should have many completions, got ${labels.length}`
            );
        } finally {
            await cleanup();
        }
    });

    test('should provide wire name completions', async function () {
        this.timeout(15000);
        const source = 'wire osc = cycle~(440);\nwire gain = gain~();\n';
        const { doc, cleanup } = await openFlutmaxSource(source);
        try {
            await delay(2000);
            // Request completions inside gain~() arguments
            const position = new vscode.Position(1, 18);
            const completions = await vscode.commands.executeCommand<vscode.CompletionList>(
                'vscode.executeCompletionItemProvider', doc.uri, position
            );
            assert.ok(completions, 'should return completions');
            const labels = extractLabels(completions);
            assert.ok(
                labels.includes('osc'),
                'should include defined wire name "osc"'
            );
        } finally {
            await cleanup();
        }
    });

    test('should provide in-port name completions', async function () {
        this.timeout(15000);
        const source = 'in 0 (freq): float;\nin 1 (gain): float;\nwire x = \n';
        const { doc, cleanup } = await openFlutmaxSource(source);
        try {
            await delay(2000);
            const position = new vscode.Position(2, 9);
            const completions = await vscode.commands.executeCommand<vscode.CompletionList>(
                'vscode.executeCompletionItemProvider', doc.uri, position
            );
            assert.ok(completions, 'should return completions');
            const labels = extractLabels(completions);
            assert.ok(
                labels.includes('freq'),
                'should include in-port name "freq"'
            );
            assert.ok(
                labels.includes('gain'),
                'should include in-port name "gain"'
            );
        } finally {
            await cleanup();
        }
    });
});
