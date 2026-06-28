/**
 * tree-sitter-cmd — a grammar for Windows cmd.exe / batch (.bat, .cmd) scripts.
 *
 * Design notes live in GRAMMAR_DESIGN.md. The short version:
 *
 *   - cmd.exe has no whole-file grammar; each physical line is expanded, parsed
 *     and executed in turn. We model the whole file as one CST for tooling.
 *   - `extras` is `[ \t]` plus caret line-continuations only. Newlines are
 *     significant statement terminators.
 *   - A "word" (command name / argument) is a run of adjacent fragments: bare
 *     text, quoted strings, caret escapes and expansions. Adjacency without
 *     whitespace is expressed with the zero-width external `_concat` token, so
 *     `C:\%ROOT%\bin` is one argument but `a b` is two.
 *   - Expansions (`%VAR%`, `!VAR!`, `%1`, `%*`, `%~dp0`, `%%i`, `%%`) are single
 *     tokens, so maximal munch prefers a real expansion over a stray `%`/`!`
 *     with no parser ambiguity, while a lone `%` still falls back to literal.
 *   - Keywords are case-insensitive and given elevated token precedence so they
 *     win ties against barewords, while longer barewords still win by maximal
 *     munch (so `rem` is a comment but `remote` is a command).
 *   - Command operators bind, lowest to highest: `&` < `||` < `&&` < `|`
 *     (matching ReactOS's `OpString` ordering).
 *
 * @author tree-sitter-cmd contributors
 * @license MIT
 */

/* eslint-disable no-undef */

// Keyword tokens win ties against barewords (but not longer maximal munches).
const KW_PREC = 2;

// Operator precedences, lowest to highest binding.
const PREC = {
  SEQ: 1, // &
  OR: 2, // ||
  AND: 3, // &&
  PIPE: 4, // |
  COMMAND: 5, // a simple command greedily owns its argument tail
};

// Modifier letters for %~ / %%~ expansions (case-insensitive).
const TILDE_MODS = '[dpnxfsatzDPNXFSATZ]*';
// An optional `$ENV:` PATH-search clause inside a %~ expansion.
const PATH_SEARCH = '(?:\\$[A-Za-z_][^:\\r\\n]*:)?';

/** Build a case-insensitive regexp source for a literal word. */
function ciSource(word) {
  return word
    .split('')
    .map((ch) =>
      /[a-zA-Z]/.test(ch)
        ? `[${ch.toLowerCase()}${ch.toUpperCase()}]`
        : ch.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&'),
    )
    .join('');
}

/** Case-insensitive keyword as a single elevated-precedence token. */
function ci(word) {
  return token(prec(KW_PREC, new RegExp(ciSource(word))));
}

module.exports = grammar({
  name: 'cmd',

  externals: ($) => [$._concat],

  extras: ($) => [/[ \t]/, $.line_continuation],

  conflicts: ($) => [],

  rules: {
    // A batch file is a sequence of physical lines. Every line is terminated by
    // a newline except, optionally, the last one.
    program: ($) => seq(repeat($._line), optional($._line_content)),

    _line: ($) => seq(optional($._line_content), $._newline),

    _line_content: ($) => choice($._statement, $.label, $.colon_comment),

    _newline: ($) => token(/\r?\n/),

    // A caret immediately before a newline splices the next physical line.
    line_continuation: ($) => token(/\^\r?\n[ \t]*/),

    // ---------------------------------------------------------------------
    // Statements and the command-operator ladder.
    // ---------------------------------------------------------------------
    _statement: ($) =>
      choice($._unit, $.seq_list, $.or_list, $.and_list, $.pipeline),

    seq_list: ($) =>
      prec.left(
        PREC.SEQ,
        seq(
          field('left', $._statement),
          '&',
          optional(field('right', $._statement)),
        ),
      ),

    or_list: ($) =>
      prec.left(
        PREC.OR,
        seq(field('left', $._statement), '||', field('right', $._statement)),
      ),

    and_list: ($) =>
      prec.left(
        PREC.AND,
        seq(field('left', $._statement), '&&', field('right', $._statement)),
      ),

    pipeline: ($) =>
      prec.left(
        PREC.PIPE,
        seq(field('left', $._statement), '|', field('right', $._statement)),
      ),

    _unit: ($) => choice($.command, $.block, $.rem_comment),

    // A parenthesised compound. Newlines inside act like `&`.
    block: ($) =>
      prec.right(
        seq(
          '(',
          optional($._block_body),
          ')',
          repeat(field('redirect', $.redirection)),
        ),
      ),
    _block_body: ($) =>
      seq(
        repeat($._newline),
        $._statement,
        repeat(seq(repeat1($._newline), $._statement)),
        repeat($._newline),
      ),

    command: ($) =>
      prec.right(
        PREC.COMMAND,
        seq(
          optional(field('quiet', $.quiet)),
          repeat(field('redirect', $.redirection)),
          field('name', $.command_name),
          repeat(
            choice(field('argument', $.argument), field('redirect', $.redirection)),
          ),
        ),
      ),

    // The `@` echo-suppress prefix.
    quiet: ($) => '@',

    // ---------------------------------------------------------------------
    // Redirections
    // ---------------------------------------------------------------------
    redirection: ($) => choice($.redirect_file, $.redirect_dup),

    // `>`, `>>`, `<`, with an optional immediately-preceding fd digit.
    redirect_file: ($) =>
      seq(field('op', $.redirect_operator), field('target', $.argument)),
    redirect_operator: ($) => token(/[0-9]?(?:>>|>|<)/),

    // Handle duplication: `2>&1`, `>&2`, `<&3`.
    redirect_dup: ($) =>
      seq(
        field('op', $.redirect_dup_operator),
        field('target', alias(token.immediate(/[0-9]/), $.file_descriptor)),
      ),
    redirect_dup_operator: ($) => token(/[0-9]?[<>]&/),

    // ---------------------------------------------------------------------
    // Words: command names and arguments are runs of adjacent fragments.
    // ---------------------------------------------------------------------
    command_name: ($) =>
      seq($._cmd_lead, repeat(seq($._concat, $._fragment))),

    // The first fragment of a command name may not begin with `@` (quiet) or
    // `:` (label), but may otherwise contain `:` (drive letters, `c:\...`).
    _cmd_lead: ($) => choice($._cmd_text, $.string, $._expansion),
    _cmd_text: ($) => token(/[^ \t\r\n&|<>()^"%!@:][^ \t\r\n&|<>()^"%!]*/),

    argument: ($) => seq($._fragment, repeat(seq($._concat, $._fragment))),

    _fragment: ($) =>
      choice(
        $.text,
        $.string,
        $.escape_sequence,
        $._expansion,
        // A `%` or `!` that is not part of an expansion is literal text.
        alias($._stray_sigil, $.text),
      ),

    text: ($) => token(/[^ \t\r\n&|<>()^"%!]+/),
    _stray_sigil: ($) => token(/[%!]/),

    // A caret escapes the single following (non-newline) character.
    escape_sequence: ($) => token(/\^[^\r\n]/),

    // cmd does not strip quotes; they only group. An unterminated quote runs to
    // end of line. Modelled as a single token (the interior is opaque for now —
    // cmd does still expand %VAR% inside quotes, but we do not sub-node it yet).
    string: ($) => token(/"[^"\r\n]*"?/),

    // ---------------------------------------------------------------------
    // Expansions (each is a single token; see header note)
    // ---------------------------------------------------------------------
    _expansion: ($) =>
      choice(
        $.variable,
        $.delayed_variable,
        $.parameter,
        $.all_arguments,
        $.parameter_tilde,
        $.loop_variable,
        $.percent_literal,
      ),

    // %NAME% and %NAME:...% (substring / substitution). The leading character
    // is not a digit/`~`/`*` so positional, tilde and `%*` forms win instead.
    variable: ($) => token(/%[^%0-9~*\r\n][^%\r\n]*%/),
    // !NAME! delayed expansion (always recognised; literal unless delayed
    // expansion is enabled at runtime).
    delayed_variable: ($) => token(/![^!\r\n]+!/),

    // %0..%9 positional parameters.
    parameter: ($) => token(/%[0-9]/),
    // %* all command-line arguments.
    all_arguments: ($) => token(/%\*/),
    // %~modifiers[$ENV:]N argument modifiers, e.g. %~dp0, %~$PATH:1.
    parameter_tilde: ($) =>
      token(new RegExp('%~' + TILDE_MODS + PATH_SEARCH + '[0-9]')),
    // %%x FOR loop variable (batch context), optionally with ~modifiers.
    loop_variable: ($) =>
      token(new RegExp('%%(?:~' + TILDE_MODS + PATH_SEARCH + ')?[A-Za-z]')),
    // %% literal percent sign.
    percent_literal: ($) => token(/%%/),

    // ---------------------------------------------------------------------
    // Labels and comments
    // ---------------------------------------------------------------------
    label: ($) =>
      seq(
        token(/:/),
        field('name', alias($._label_name, $.label_name)),
        optional($._label_tail),
      ),
    _label_name: ($) => token.immediate(/[^ \t\r\n:+;=,&|<>()]+/),
    _label_tail: ($) => alias(token(/[^\r\n]+/), $.label_text),

    rem_comment: ($) =>
      seq(ci('rem'), optional(alias($._line_text, $.comment_text))),
    colon_comment: ($) =>
      seq(token(/::/), optional(alias($._line_text, $.comment_text))),
    _line_text: ($) => token(/[^\r\n]+/),
  },
});
