import * as assert from 'assert';
import * as fs from 'fs';
import * as path from 'path';

suite('Language Configuration', () => {
    let config: any;

    suiteSetup(() => {
        const configPath = path.join(__dirname, '../../../language-configuration.json');
        config = JSON.parse(fs.readFileSync(configPath, 'utf-8'));
    });

    test('is valid JSON', () => {
        assert.ok(config, 'should parse as valid JSON');
    });

    suite('Comments', () => {
        test('defines line comment as //', () => {
            assert.ok(config.comments, 'should have comments section');
            assert.strictEqual(config.comments.lineComment, '//');
        });

        test('does not define block comments', () => {
            // flutmax only supports line comments
            assert.strictEqual(
                config.comments.blockComment,
                undefined,
                'flutmax should not define block comments'
            );
        });
    });

    suite('Brackets', () => {
        test('defines bracket pairs', () => {
            assert.ok(Array.isArray(config.brackets), 'brackets should be an array');
            assert.ok(config.brackets.length > 0, 'should have at least one bracket pair');
        });

        test('includes parentheses pair', () => {
            const hasParens = config.brackets.some(
                (pair: string[]) => pair[0] === '(' && pair[1] === ')'
            );
            assert.ok(hasParens, 'should include () bracket pair');
        });

        test('includes square brackets pair', () => {
            const hasBrackets = config.brackets.some(
                (pair: string[]) => pair[0] === '[' && pair[1] === ']'
            );
            assert.ok(hasBrackets, 'should include [] bracket pair');
        });
    });

    suite('Auto-closing pairs', () => {
        test('defines auto-closing pairs', () => {
            assert.ok(
                Array.isArray(config.autoClosingPairs),
                'autoClosingPairs should be an array'
            );
        });

        test('auto-closes parentheses', () => {
            const hasParens = config.autoClosingPairs.some(
                (pair: any) => pair.open === '(' && pair.close === ')'
            );
            assert.ok(hasParens, 'should auto-close parentheses');
        });

        test('auto-closes square brackets', () => {
            const hasBrackets = config.autoClosingPairs.some(
                (pair: any) => pair.open === '[' && pair.close === ']'
            );
            assert.ok(hasBrackets, 'should auto-close square brackets');
        });

        test('auto-closes double quotes', () => {
            const hasQuotes = config.autoClosingPairs.some(
                (pair: any) => pair.open === '"' && pair.close === '"'
            );
            assert.ok(hasQuotes, 'should auto-close double quotes');
        });

        test('double quotes do not auto-close inside strings', () => {
            const quotePair = config.autoClosingPairs.find(
                (pair: any) => pair.open === '"' && pair.close === '"'
            );
            assert.ok(quotePair, 'should have quote pair');
            assert.ok(
                quotePair.notIn && quotePair.notIn.includes('string'),
                'double quotes should not auto-close inside strings'
            );
        });
    });

    suite('Surrounding pairs', () => {
        test('defines surrounding pairs', () => {
            assert.ok(
                Array.isArray(config.surroundingPairs),
                'surroundingPairs should be an array'
            );
        });

        test('includes parentheses for surrounding', () => {
            const hasParens = config.surroundingPairs.some(
                (pair: string[]) => pair[0] === '(' && pair[1] === ')'
            );
            assert.ok(hasParens, 'should surround with parentheses');
        });

        test('includes square brackets for surrounding', () => {
            const hasBrackets = config.surroundingPairs.some(
                (pair: string[]) => pair[0] === '[' && pair[1] === ']'
            );
            assert.ok(hasBrackets, 'should surround with square brackets');
        });

        test('includes double quotes for surrounding', () => {
            const hasQuotes = config.surroundingPairs.some(
                (pair: string[]) => pair[0] === '"' && pair[1] === '"'
            );
            assert.ok(hasQuotes, 'should surround with double quotes');
        });
    });

    suite('Word pattern', () => {
        test('defines a word pattern', () => {
            assert.ok(config.wordPattern, 'should define wordPattern');
        });

        test('matches plain identifiers', () => {
            const pattern = new RegExp(config.wordPattern);
            assert.ok(pattern.test('freq'), 'should match "freq"');
            assert.ok(pattern.test('cutoff'), 'should match "cutoff"');
            assert.ok(pattern.test('my_var'), 'should match "my_var"');
            assert.ok(pattern.test('x123'), 'should match "x123"');
        });

        test('matches tilde identifiers (signal objects)', () => {
            const pattern = new RegExp(config.wordPattern);
            assert.ok(pattern.test('cycle~'), 'should match "cycle~"');
            assert.ok(pattern.test('mul~'), 'should match "mul~"');
            assert.ok(pattern.test('biquad~'), 'should match "biquad~"');
            assert.ok(pattern.test('dac~'), 'should match "dac~"');
            assert.ok(pattern.test('phasor~'), 'should match "phasor~"');
        });

        test('treats tilde identifier as single word', () => {
            const pattern = new RegExp(config.wordPattern);
            const match = 'cycle~'.match(pattern);
            assert.ok(match, 'should match cycle~');
            assert.strictEqual(match![0], 'cycle~', 'full match should be "cycle~"');
        });
    });
});
