; Keywords
["wire" "in" "out"] @keyword

; Types
(type_name) @type

; Direction
(direction) @keyword

; Identifiers
(plain_identifier) @variable

; Tilde-suffixed objects
(tilde_identifier) @function

; Object call name
(object_call
  object: (object_name) @function.call)

; Port declaration name
(port_declaration
  name: (plain_identifier) @variable.parameter)

; Numbers
(number) @number
(integer) @number

; Strings
(string) @string

; Comments
(comment) @comment

; Brackets
["(" ")" "[" "]"] @punctuation.bracket
["," "." ":"] @punctuation.delimiter
[";" "="] @punctuation.special
