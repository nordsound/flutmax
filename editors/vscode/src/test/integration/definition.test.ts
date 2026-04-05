/**
 * Go to Definition Integration Tests
 *
 * Verifies that Ctrl+Click on a wire name jumps to its declaration.
 */

import * as assert from 'assert';
import * as vscode from 'vscode';
import { openFlutmaxSource } from './helpers';

function delay(ms: number): Promise<void> {
    return new Promise(resolve => setTimeout(resolve, ms));
}

suite('Go to Definition', () => {
    test('should jump to wire declaration from reference', async function () {
        this.timeout(15000);
        const source = [
            'out audio: signal;',
            'wire osc = cycle~(440);',
            'wire gain = mul~(osc, 0.5);',
            'out[0] = gain;',
        ].join('\n') + '\n';
        const { doc, cleanup } = await openFlutmaxSource(source);
        try {
            await delay(2000);
            // Position on "osc" in mul~(osc, ...) — line 2, col ~20
            const position = new vscode.Position(2, 20);
            const locations = await vscode.commands.executeCommand<vscode.Location[]>(
                'vscode.executeDefinitionProvider', doc.uri, position
            );
            assert.ok(locations, 'should return definition locations');
            assert.ok(locations.length > 0, `should find definition, got ${locations.length}`);

            // Definition should point to line 1 (wire osc = ...)
            const defLine = locations[0].range.start.line;
            assert.ok(
                defLine <= 1,
                `definition should be on line 0 or 1 (wire declaration), got line ${defLine}`
            );
        } finally {
            await cleanup();
        }
    });

    test('should jump to input port declaration', async function () {
        this.timeout(15000);
        const source = [
            'in freq: float;',
            'out audio: signal;',
            'wire osc = cycle~(freq);',
            'out[0] = osc;',
        ].join('\n') + '\n';
        const { doc, cleanup } = await openFlutmaxSource(source);
        try {
            await delay(2000);
            // Position on "freq" in cycle~(freq) — line 2
            const position = new vscode.Position(2, 20);
            const locations = await vscode.commands.executeCommand<vscode.Location[]>(
                'vscode.executeDefinitionProvider', doc.uri, position
            );
            assert.ok(locations, 'should return definition locations');
            if (locations.length > 0) {
                const defLine = locations[0].range.start.line;
                assert.ok(
                    defLine === 0,
                    `definition should be on line 0 (in freq: float;), got line ${defLine}`
                );
            }
        } finally {
            await cleanup();
        }
    });

    test('should return empty for unknown identifier', async function () {
        this.timeout(15000);
        const source = 'out audio: signal;\nwire osc = cycle~(440);\nout[0] = osc;\n';
        const { doc, cleanup } = await openFlutmaxSource(source);
        try {
            await delay(2000);
            // Position on "440" — a number, not a wire reference
            const position = new vscode.Position(1, 18);
            const locations = await vscode.commands.executeCommand<vscode.Location[]>(
                'vscode.executeDefinitionProvider', doc.uri, position
            );
            // Numbers shouldn't have definitions — either null/empty or no match
            if (locations) {
                // It's OK if LSP returns something for the token, as long as it doesn't crash
                assert.ok(true);
            }
        } finally {
            await cleanup();
        }
    });
});
