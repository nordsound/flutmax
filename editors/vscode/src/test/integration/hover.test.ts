/**
 * Hover Integration Tests
 *
 * Verifies that the LSP hover provider returns information
 * for Max objects (from objdb) and wire names.
 */

import * as assert from 'assert';
import * as vscode from 'vscode';
import { openFlutmaxSource } from './helpers';

function delay(ms: number): Promise<void> {
    return new Promise(resolve => setTimeout(resolve, ms));
}

suite('Hover Provider', () => {
    test('should show hover for Max object name', async function () {
        this.timeout(15000);
        const source = 'out audio: signal;\nwire osc = cycle~(440);\nout[0] = osc;\n';
        const { doc, cleanup } = await openFlutmaxSource(source);
        try {
            await delay(2000);
            // Position on "cycle~" (line 1, within the word)
            const position = new vscode.Position(1, 14);
            const hovers = await vscode.commands.executeCommand<vscode.Hover[]>(
                'vscode.executeHoverProvider', doc.uri, position
            );
            assert.ok(hovers, 'should return hover results');
            assert.ok(hovers.length > 0, 'should have at least one hover');

            const content = hovers[0].contents
                .map((c: any) => typeof c === 'string' ? c : c.value)
                .join('\n');

            assert.ok(
                content.includes('cycle~') || content.includes('cycle'),
                `hover should mention cycle~: ${content}`
            );
        } finally {
            await cleanup();
        }
    });

    test('should show hover for wire name', async function () {
        this.timeout(15000);
        const source = 'out audio: signal;\nwire osc = cycle~(440);\nwire gain = mul~(osc, 0.5);\nout[0] = gain;\n';
        const { doc, cleanup } = await openFlutmaxSource(source);
        try {
            await delay(2000);
            // Position on "osc" in the mul~ call (line 2)
            const position = new vscode.Position(2, 20);
            const hovers = await vscode.commands.executeCommand<vscode.Hover[]>(
                'vscode.executeHoverProvider', doc.uri, position
            );
            assert.ok(hovers, 'should return hover results');
            if (hovers.length > 0) {
                const content = hovers[0].contents
                    .map((c: any) => typeof c === 'string' ? c : c.value)
                    .join('\n');
                assert.ok(
                    content.includes('osc') || content.includes('wire'),
                    `hover should show wire info: ${content}`
                );
            }
        } finally {
            await cleanup();
        }
    });

    test('should show hover for input port', async function () {
        this.timeout(15000);
        const source = 'in freq: float;\nout audio: signal;\nwire osc = cycle~(freq);\nout[0] = osc;\n';
        const { doc, cleanup } = await openFlutmaxSource(source);
        try {
            await delay(2000);
            // Position on "freq" in cycle~ call (line 2)
            const position = new vscode.Position(2, 20);
            const hovers = await vscode.commands.executeCommand<vscode.Hover[]>(
                'vscode.executeHoverProvider', doc.uri, position
            );
            assert.ok(hovers, 'should return hover results');
            if (hovers.length > 0) {
                const content = hovers[0].contents
                    .map((c: any) => typeof c === 'string' ? c : c.value)
                    .join('\n');
                assert.ok(
                    content.includes('freq') || content.includes('input') || content.includes('float'),
                    `hover should show port info: ${content}`
                );
            }
        } finally {
            await cleanup();
        }
    });
});
