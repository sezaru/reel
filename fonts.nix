# The fonts reel falls back to, the same on every host: Noto for every script, CJK, colour
# emoji, and the generic families (sans-serif, serif, monospace) mapped to Noto, so they
# have every weight. The host's own fonts and font settings are left out on purpose: a
# scene renders the same anywhere (its own fonts come from its `fonts` list).
# (`FONTCONFIG_FILE` points at this.)
{ pkgs }:
let
  dirs = with pkgs; [ noto-fonts noto-fonts-cjk-sans noto-fonts-cjk-serif noto-fonts-color-emoji ];
  alias = generic: families: ''
    <alias binding="strong"><family>${generic}</family><prefer>${pkgs.lib.concatMapStrings (f: "<family>${f}</family>") families}</prefer></alias>
  '';
in
pkgs.writeText "reel-fonts.conf" ''
  <?xml version="1.0"?>
  <!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
  <fontconfig>
    ${pkgs.lib.concatMapStrings (d: "<dir>${d}/share/fonts</dir>\n") dirs}
    <cachedir prefix="xdg">fontconfig</cachedir>
    ${alias "sans-serif" [ "Noto Sans" "Noto Sans CJK SC" "Noto Color Emoji" ]}
    ${alias "serif" [ "Noto Serif" "Noto Serif CJK SC" "Noto Color Emoji" ]}
    ${alias "monospace" [ "Noto Sans Mono" "Noto Sans Mono CJK SC" ]}
    ${alias "system-ui" [ "Noto Sans" ]}
    ${alias "emoji" [ "Noto Color Emoji" ]}
  </fontconfig>
''
