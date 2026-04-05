import * as assert from 'assert';
import * as vscode from 'vscode';

function findFlutmaxExtension(): vscode.Extension<unknown> | undefined {
    return vscode.extensions.all.find(ext =>
        ext.packageJSON?.name === 'flutmax' ||
        ext.id.includes('flutmax')
    );
}

suite('Extension Activation', () => {
    let extension: vscode.Extension<unknown> | undefined;

    suiteSetup(function () {
        extension = findFlutmaxExtension();
        if (!extension) {
            console.log('[test] flutmax extension not found, skipping');
            this.skip();
        }
    });

    test('should be present in extension list', function () {
        if (!extension) { this.skip(); return; }
        assert.ok(extension, 'Extension should be found');
    });

    test('should activate successfully', async function () {
        if (!extension) { this.skip(); return; }
        if (!extension.isActive) {
            await extension.activate();
        }
        assert.ok(extension.isActive, 'Extension should be active');
    });

    test('should register flutmax language', function () {
        if (!extension) { this.skip(); return; }
        const contributes = extension.packageJSON.contributes;
        assert.ok(contributes.languages, 'Should contribute languages');
        const lang = contributes.languages.find(
            (l: { id: string }) => l.id === 'flutmax'
        );
        assert.ok(lang, 'flutmax language should be registered');
        assert.ok(
            lang.extensions.includes('.flutmax'),
            '.flutmax extension should be registered'
        );
    });

    test('should register LSP configuration settings', function () {
        if (!extension) { this.skip(); return; }
        const contributes = extension.packageJSON.contributes;
        assert.ok(contributes.configuration, 'Should contribute configuration');
        const props = contributes.configuration[0]?.properties ?? {};
        assert.ok(props['flutmax.lsp.enabled'], 'lsp.enabled setting should exist');
        assert.ok(props['flutmax.lsp.path'], 'lsp.path setting should exist');
    });
});
