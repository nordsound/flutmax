import * as assert from 'assert';
import * as fs from 'fs';
import * as path from 'path';

suite('Snippets', () => {
    let snippets: Record<string, any>;

    suiteSetup(() => {
        const snippetsPath = path.join(__dirname, '../../../snippets/flutmax.json');
        snippets = JSON.parse(fs.readFileSync(snippetsPath, 'utf-8'));
    });

    test('is valid JSON with at least one snippet', () => {
        const keys = Object.keys(snippets);
        assert.ok(keys.length > 0, 'should have at least one snippet');
    });

    test('all snippets have required fields', () => {
        for (const [name, snippet] of Object.entries(snippets)) {
            assert.ok(
                (snippet as any).prefix,
                `"${name}" missing prefix`
            );
            assert.ok(
                (snippet as any).body,
                `"${name}" missing body`
            );
            assert.ok(
                (snippet as any).description,
                `"${name}" missing description`
            );
        }
    });

    test('all snippet bodies are arrays of strings', () => {
        for (const [name, snippet] of Object.entries(snippets)) {
            const body = (snippet as any).body;
            assert.ok(Array.isArray(body), `"${name}" body should be an array`);
            for (const line of body) {
                assert.strictEqual(
                    typeof line, 'string',
                    `"${name}" body contains non-string element`
                );
            }
        }
    });

    test('all snippet prefixes are non-empty strings', () => {
        for (const [name, snippet] of Object.entries(snippets)) {
            const prefix = (snippet as any).prefix;
            assert.strictEqual(
                typeof prefix, 'string',
                `"${name}" prefix should be a string`
            );
            assert.ok(prefix.length > 0, `"${name}" prefix should not be empty`);
        }
    });

    test('snippet prefixes are unique', () => {
        const prefixes: string[] = [];
        for (const [name, snippet] of Object.entries(snippets)) {
            const prefix = (snippet as any).prefix;
            assert.ok(
                !prefixes.includes(prefix),
                `duplicate prefix "${prefix}" found in "${name}"`
            );
            prefixes.push(prefix);
        }
    });

    suite('Essential snippets', () => {
        function findSnippetByPrefix(prefix: string): any | undefined {
            return Object.values(snippets).find(
                (s: any) => s.prefix === prefix
            );
        }

        test('has "in" snippet for input port declaration', () => {
            const snippet = findSnippetByPrefix('in');
            assert.ok(snippet, 'missing "in" snippet');
        });

        test('has "out" snippet for output port declaration', () => {
            const snippet = findSnippetByPrefix('out');
            assert.ok(snippet, 'missing "out" snippet');
        });

        test('has "wire" snippet for wire declaration', () => {
            const snippet = findSnippetByPrefix('wire');
            assert.ok(snippet, 'missing "wire" snippet');
        });

        test('has "wire~" snippet for signal wire declaration', () => {
            const snippet = findSnippetByPrefix('wire~');
            assert.ok(snippet, 'missing "wire~" snippet');
        });

        test('has "synth" snippet for synthesizer template', () => {
            const snippet = findSnippetByPrefix('synth');
            assert.ok(snippet, 'missing "synth" snippet');
        });

        test('has "filter" snippet for filter template', () => {
            const snippet = findSnippetByPrefix('filter');
            assert.ok(snippet, 'missing "filter" snippet');
        });

        test('has "outa" snippet for output assignment', () => {
            const snippet = findSnippetByPrefix('outa');
            assert.ok(snippet, 'missing "outa" snippet');
        });

        test('has "stereo" snippet for stereo output template', () => {
            const snippet = findSnippetByPrefix('stereo');
            assert.ok(snippet, 'missing "stereo" snippet');
        });
    });

    suite('Snippet body content', () => {
        test('"in" snippet produces port declaration syntax', () => {
            const snippet = Object.values(snippets).find(
                (s: any) => s.prefix === 'in'
            ) as any;
            const body = snippet.body.join('\n');
            // After expanding, should contain "in", a number, parenthesized name, colon, type, semicolon
            assert.ok(body.includes('in'), 'should contain "in" keyword');
            assert.ok(body.includes(':'), 'should contain colon for type');
            assert.ok(body.includes(';'), 'should contain semicolon');
        });

        test('"wire" snippet produces wire declaration syntax', () => {
            const snippet = Object.values(snippets).find(
                (s: any) => s.prefix === 'wire'
            ) as any;
            const body = snippet.body.join('\n');
            assert.ok(body.includes('wire'), 'should contain "wire" keyword');
            assert.ok(body.includes('='), 'should contain assignment');
            assert.ok(body.includes(';'), 'should contain semicolon');
        });

        test('"wire~" snippet body includes tilde object', () => {
            const snippet = Object.values(snippets).find(
                (s: any) => s.prefix === 'wire~'
            ) as any;
            const body = snippet.body.join('\n');
            assert.ok(body.includes('~'), 'should contain tilde for signal object');
        });

        test('"synth" snippet includes oscillator and output', () => {
            const snippet = Object.values(snippets).find(
                (s: any) => s.prefix === 'synth'
            ) as any;
            const body = snippet.body.join('\n');
            assert.ok(body.includes('cycle~'), 'should contain cycle~ oscillator');
            assert.ok(body.includes('out[0]'), 'should contain output assignment');
            assert.ok(body.includes('in 0'), 'should contain input port');
        });

        test('"filter" snippet includes biquad~ and multiple ins', () => {
            const snippet = Object.values(snippets).find(
                (s: any) => s.prefix === 'filter'
            ) as any;
            const body = snippet.body.join('\n');
            assert.ok(body.includes('biquad~'), 'should contain biquad~ filter');
            assert.ok(body.includes('in 0'), 'should contain input port 0');
            assert.ok(body.includes('in 1'), 'should contain input port 1');
            assert.ok(body.includes('in 2'), 'should contain input port 2');
        });
    });
});
