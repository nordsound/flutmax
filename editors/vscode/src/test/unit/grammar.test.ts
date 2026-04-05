import * as assert from 'assert';
import * as fs from 'fs';
import * as path from 'path';

suite('TextMate Grammar', () => {
    let grammar: any;

    suiteSetup(() => {
        const grammarPath = path.join(__dirname, '../../../syntaxes/flutmax.tmLanguage.json');
        grammar = JSON.parse(fs.readFileSync(grammarPath, 'utf-8'));
    });

    test('is valid JSON with required top-level fields', () => {
        assert.ok(grammar.name, 'missing name');
        assert.ok(grammar.scopeName, 'missing scopeName');
        assert.ok(grammar.patterns, 'missing patterns');
        assert.ok(grammar.repository, 'missing repository');
    });

    test('has correct scope name', () => {
        assert.strictEqual(grammar.scopeName, 'source.flutmax');
    });

    test('has correct language name', () => {
        assert.strictEqual(grammar.name, 'flutmax');
    });

    test('top-level patterns reference existing repository entries', () => {
        const repoKeys = Object.keys(grammar.repository);
        for (const pattern of grammar.patterns) {
            if (pattern.include) {
                const ref = pattern.include.replace('#', '');
                assert.ok(
                    repoKeys.includes(ref),
                    `top-level pattern references non-existent repository entry: ${ref}`
                );
            }
        }
    });

    test('has required pattern categories', () => {
        const repo = grammar.repository;
        assert.ok(repo.comment, 'missing comment pattern');
        assert.ok(repo.keywords, 'missing keywords pattern');
        assert.ok(repo.types, 'missing types pattern');
        assert.ok(repo['port-declaration'], 'missing port-declaration pattern');
        assert.ok(repo['wire-declaration'], 'missing wire-declaration pattern');
        assert.ok(repo['out-assignment'], 'missing out-assignment pattern');
        assert.ok(repo['direct-connection'], 'missing direct-connection pattern');
        assert.ok(repo.expression, 'missing expression pattern');
        assert.ok(repo.number, 'missing number pattern');
        assert.ok(repo.string, 'missing string pattern');
        assert.ok(repo.punctuation, 'missing punctuation pattern');
    });

    suite('Comment pattern', () => {
        test('matches line comments', () => {
            const commentPattern = new RegExp(grammar.repository.comment.match);
            assert.ok(commentPattern.test('// this is a comment'));
            assert.ok(commentPattern.test('// wire osc = cycle~(440);'));
        });

        test('has correct scope', () => {
            assert.strictEqual(
                grammar.repository.comment.name,
                'comment.line.double-slash.flutmax'
            );
        });
    });

    suite('Keywords pattern', () => {
        test('matches wire, in, out keywords', () => {
            const keywordsPattern = new RegExp(grammar.repository.keywords.match);
            assert.ok(keywordsPattern.test('wire'));
            assert.ok(keywordsPattern.test('in'));
            assert.ok(keywordsPattern.test('out'));
        });

        test('does not match partial words', () => {
            const keywordsPattern = new RegExp(grammar.repository.keywords.match);
            assert.ok(!keywordsPattern.test('wired'));
            assert.ok(!keywordsPattern.test('input'));
            assert.ok(!keywordsPattern.test('output'));
        });

        test('has correct scope', () => {
            assert.strictEqual(
                grammar.repository.keywords.name,
                'keyword.control.flutmax'
            );
        });
    });

    suite('Types pattern', () => {
        test('matches all flutmax types', () => {
            const typesPattern = new RegExp(grammar.repository.types.match);
            assert.ok(typesPattern.test('signal'), 'should match signal');
            assert.ok(typesPattern.test('float'), 'should match float');
            assert.ok(typesPattern.test('int'), 'should match int');
            assert.ok(typesPattern.test('bang'), 'should match bang');
            assert.ok(typesPattern.test('list'), 'should match list');
            assert.ok(typesPattern.test('symbol'), 'should match symbol');
        });

        test('has correct scope', () => {
            assert.strictEqual(
                grammar.repository.types.name,
                'support.type.flutmax'
            );
        });
    });

    suite('Port declaration pattern', () => {
        test('matches input port declarations', () => {
            const portPattern = new RegExp(grammar.repository['port-declaration'].match);
            assert.ok(portPattern.test('in 0 (freq): float;'));
            assert.ok(portPattern.test('in 1 (cutoff): float;'));
            assert.ok(portPattern.test('in 0 (input_sig): signal;'));
        });

        test('matches output port declarations', () => {
            const portPattern = new RegExp(grammar.repository['port-declaration'].match);
            assert.ok(portPattern.test('out 0 (audio): signal;'));
            assert.ok(portPattern.test('out 1 (highpass): signal;'));
        });

        test('captures all groups correctly', () => {
            const portPattern = new RegExp(grammar.repository['port-declaration'].match);
            const match = 'in 0 (freq): float;'.match(portPattern);
            assert.ok(match, 'pattern should match');
            assert.strictEqual(match![1], 'in');
            assert.strictEqual(match![2], '0');
            assert.strictEqual(match![3], 'freq');
            assert.strictEqual(match![4], 'float');
        });
    });

    suite('Tilde identifier pattern', () => {
        test('matches signal objects', () => {
            const tildePattern = new RegExp(grammar.repository['tilde-identifier'].match);
            assert.ok(tildePattern.test('cycle~'), 'should match cycle~');
            assert.ok(tildePattern.test('mul~'), 'should match mul~');
            assert.ok(tildePattern.test('biquad~'), 'should match biquad~');
            assert.ok(tildePattern.test('phasor~'), 'should match phasor~');
            assert.ok(tildePattern.test('dac~'), 'should match dac~');
        });

        test('has correct scope', () => {
            assert.strictEqual(
                grammar.repository['tilde-identifier'].name,
                'entity.name.function.tilde.flutmax'
            );
        });
    });

    suite('Number patterns', () => {
        test('matches integer numbers', () => {
            const intPattern = new RegExp(grammar.repository.number.patterns[1].match);
            assert.ok(intPattern.test('440'));
            assert.ok(intPattern.test('0'));
            assert.ok(intPattern.test('48000'));
        });

        test('matches float numbers', () => {
            const floatPattern = new RegExp(grammar.repository.number.patterns[0].match);
            assert.ok(floatPattern.test('3.14'));
            assert.ok(floatPattern.test('0.5'));
            assert.ok(floatPattern.test('440.0'));
        });

        test('float pattern has correct scope', () => {
            assert.strictEqual(
                grammar.repository.number.patterns[0].name,
                'constant.numeric.float.flutmax'
            );
        });

        test('integer pattern has correct scope', () => {
            assert.strictEqual(
                grammar.repository.number.patterns[1].name,
                'constant.numeric.integer.flutmax'
            );
        });
    });

    suite('String pattern', () => {
        test('has correct scope', () => {
            assert.strictEqual(
                grammar.repository.string.name,
                'string.quoted.double.flutmax'
            );
        });

        test('uses double-quote delimiters', () => {
            assert.strictEqual(grammar.repository.string.begin, '"');
            assert.strictEqual(grammar.repository.string.end, '"');
        });

        test('supports escape sequences', () => {
            const escapePattern = grammar.repository.string.patterns[0];
            assert.ok(escapePattern, 'should have escape pattern');
            assert.strictEqual(
                escapePattern.name,
                'constant.character.escape.flutmax'
            );
            const escapeRegex = new RegExp(escapePattern.match);
            assert.ok(escapeRegex.test('\\n'));
            assert.ok(escapeRegex.test('\\t'));
            assert.ok(escapeRegex.test('\\"'));
        });
    });

    suite('Wire declaration pattern', () => {
        test('uses begin/end pattern', () => {
            assert.ok(grammar.repository['wire-declaration'].begin, 'should have begin');
            assert.ok(grammar.repository['wire-declaration'].end, 'should have end');
        });

        test('begin matches wire keyword with name and assignment', () => {
            const beginPattern = new RegExp(grammar.repository['wire-declaration'].begin);
            assert.ok(beginPattern.test('wire osc ='));
            assert.ok(beginPattern.test('wire filtered ='));
            assert.ok(beginPattern.test('wire my_var_123 ='));
        });

        test('end matches semicolon', () => {
            assert.strictEqual(grammar.repository['wire-declaration'].end, ';');
        });
    });

    suite('Tilde call pattern', () => {
        test('begin matches signal function calls', () => {
            const tildeCallBegin = new RegExp(grammar.repository['tilde-call'].begin);
            assert.ok(tildeCallBegin.test('cycle~('));
            assert.ok(tildeCallBegin.test('biquad~('));
            assert.ok(tildeCallBegin.test('mul~('));
        });

        test('has correct begin captures for function name', () => {
            const captures = grammar.repository['tilde-call'].beginCaptures;
            assert.strictEqual(captures['1'].name, 'entity.name.function.tilde.flutmax');
            assert.strictEqual(captures['2'].name, 'punctuation.paren.open.flutmax');
        });
    });

    suite('Plain call pattern', () => {
        test('begin matches control function calls', () => {
            const plainCallBegin = new RegExp(grammar.repository['plain-call'].begin);
            assert.ok(plainCallBegin.test('pack('));
            assert.ok(plainCallBegin.test('trigger('));
            assert.ok(plainCallBegin.test('button('));
        });
    });
});
