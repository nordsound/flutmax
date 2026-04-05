import * as assert from 'assert';
import * as fs from 'fs';
import * as path from 'path';

suite('Package Manifest', () => {
    let pkg: any;

    suiteSetup(() => {
        const pkgPath = path.join(__dirname, '../../../package.json');
        pkg = JSON.parse(fs.readFileSync(pkgPath, 'utf-8'));
    });

    test('is valid JSON', () => {
        assert.ok(pkg, 'should parse as valid JSON');
    });

    suite('Extension metadata', () => {
        test('has correct name', () => {
            assert.strictEqual(pkg.name, 'flutmax');
        });

        test('has a display name', () => {
            assert.ok(pkg.displayName, 'should have displayName');
        });

        test('has a description', () => {
            assert.ok(pkg.description, 'should have description');
        });

        test('has a version', () => {
            assert.ok(pkg.version, 'should have version');
            assert.match(pkg.version, /^\d+\.\d+\.\d+$/, 'version should be semver');
        });

        test('specifies vscode engine version', () => {
            assert.ok(pkg.engines, 'should have engines');
            assert.ok(pkg.engines.vscode, 'should specify vscode engine');
        });

        test('is categorized as Programming Languages', () => {
            assert.ok(Array.isArray(pkg.categories), 'categories should be an array');
            assert.ok(
                pkg.categories.includes('Programming Languages'),
                'should include "Programming Languages" category'
            );
        });
    });

    suite('Language contribution', () => {
        test('registers flutmax language', () => {
            assert.ok(pkg.contributes, 'should have contributes');
            assert.ok(
                Array.isArray(pkg.contributes.languages),
                'should have languages array'
            );
            const lang = pkg.contributes.languages.find(
                (l: any) => l.id === 'flutmax'
            );
            assert.ok(lang, 'should register flutmax language');
        });

        test('associates .flutmax file extension', () => {
            const lang = pkg.contributes.languages.find(
                (l: any) => l.id === 'flutmax'
            );
            assert.ok(
                lang.extensions && lang.extensions.includes('.flutmax'),
                'should associate .flutmax extension'
            );
        });

        test('includes flutmax alias', () => {
            const lang = pkg.contributes.languages.find(
                (l: any) => l.id === 'flutmax'
            );
            assert.ok(
                lang.aliases && lang.aliases.length > 0,
                'should have at least one alias'
            );
        });

        test('references language configuration file', () => {
            const lang = pkg.contributes.languages.find(
                (l: any) => l.id === 'flutmax'
            );
            assert.ok(
                lang.configuration,
                'should reference language configuration'
            );
            // Verify the referenced file exists
            const configPath = path.join(
                __dirname, '../../..', lang.configuration
            );
            assert.ok(
                fs.existsSync(configPath),
                `language configuration file not found: ${lang.configuration}`
            );
        });
    });

    suite('Grammar contribution', () => {
        test('registers TextMate grammar', () => {
            assert.ok(
                Array.isArray(pkg.contributes.grammars),
                'should have grammars array'
            );
            assert.ok(
                pkg.contributes.grammars.length > 0,
                'should have at least one grammar'
            );
        });

        test('grammar targets flutmax language', () => {
            const grammar = pkg.contributes.grammars[0];
            assert.strictEqual(grammar.language, 'flutmax');
        });

        test('grammar has correct scope name', () => {
            const grammar = pkg.contributes.grammars[0];
            assert.strictEqual(grammar.scopeName, 'source.flutmax');
        });

        test('grammar references existing file', () => {
            const grammar = pkg.contributes.grammars[0];
            assert.ok(grammar.path, 'should have grammar path');
            const grammarPath = path.join(__dirname, '../../..', grammar.path);
            assert.ok(
                fs.existsSync(grammarPath),
                `grammar file not found: ${grammar.path}`
            );
        });
    });

    suite('Snippets contribution', () => {
        test('registers snippets', () => {
            assert.ok(
                Array.isArray(pkg.contributes.snippets),
                'should have snippets array'
            );
            assert.ok(
                pkg.contributes.snippets.length > 0,
                'should have at least one snippet entry'
            );
        });

        test('snippets target flutmax language', () => {
            const snippet = pkg.contributes.snippets[0];
            assert.strictEqual(snippet.language, 'flutmax');
        });

        test('snippets reference existing file', () => {
            const snippet = pkg.contributes.snippets[0];
            assert.ok(snippet.path, 'should have snippets path');
            const snippetsPath = path.join(__dirname, '../../..', snippet.path);
            assert.ok(
                fs.existsSync(snippetsPath),
                `snippets file not found: ${snippet.path}`
            );
        });
    });

    suite('Consistency checks', () => {
        test('grammar scopeName matches tmLanguage file', () => {
            const grammar = pkg.contributes.grammars[0];
            const grammarPath = path.join(__dirname, '../../..', grammar.path);
            const tmLanguage = JSON.parse(fs.readFileSync(grammarPath, 'utf-8'));
            assert.strictEqual(
                grammar.scopeName,
                tmLanguage.scopeName,
                'package.json scopeName should match tmLanguage scopeName'
            );
        });
    });
});
