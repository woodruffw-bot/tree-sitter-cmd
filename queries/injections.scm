; FOR /F command-source mode is explicit in the CST. Capture the shared inner
; node so injected Cmd excludes either active delimiter.
((for_f_command_source
  content: (for_f_command_content) @injection.content)
 (#set! injection.self))
