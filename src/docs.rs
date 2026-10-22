//! `reel docs` and `reel init`: the documentation is built into the binary, so an agent (or a
//! person) in any project can read the version it is running.
use std::path::Path;

use anyhow::Context as _;

const GUIDE: &str = include_str!("../docs/guide.md");
const SHADERS: &str = include_str!("../docs/shaders.md");
const MODELS: &str = include_str!("../docs/models.md");
const API: &str = include_str!("../js/reel.d.ts");
const EXAMPLES: &[(&str, &str)] = &[
    ("hello.ts", include_str!("../examples/hello.ts")),
    ("cards.ts", include_str!("../examples/cards.ts")),
    ("phone.ts", include_str!("../examples/phone.ts")),
    ("paper.wgsl", include_str!("../examples/paper.wgsl")),
];

const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "es2022",
    "module": "es2022",
    "moduleResolution": "bundler",
    "strict": true,
    "noEmit": true,
    "resolveJsonModule": true,
    "allowImportingTsExtensions": true,
    "types": []
  },
  "include": ["**/*.ts"]
}
"#;

pub fn print(topic: &str) -> anyhow::Result<()> {
    match topic {
        "guide" => print!("{GUIDE}"),
        "api" => print!("{API}"),
        "shaders" => print!("{SHADERS}"),
        "models" => print!("{MODELS}"),
        "examples" => {
            for (name, src) in EXAMPLES {
                println!("// ═══ examples/{name} ═══\n{src}");
            }
        }
        t => anyhow::bail!("no docs topic {t:?}: guide, api, shaders, models, examples"),
    }
    Ok(())
}

fn write_new(path: &Path, content: &str) -> anyhow::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    std::fs::write(path, content).with_context(|| format!("{}", path.display()))?;
    Ok(true)
}

pub fn init(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    // the types always track the reel that runs them
    std::fs::write(dir.join("reel.d.ts"), API)?;
    println!("wrote {}", dir.join("reel.d.ts").display());
    for (name, content) in [("tsconfig.json", TSCONFIG), ("scene.ts", EXAMPLES[0].1)] {
        let p = dir.join(name);
        if write_new(&p, content)? {
            println!("wrote {}", p.display());
        } else {
            println!("kept {} (exists)", p.display());
        }
    }
    println!("\nnext: reel preview {0}  ·  reel check {0}  ·  reel render {0}  ·  reel docs", dir.join("scene.ts").display());
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Exported values (not types) in a file: `export function NAME`, `export const NAME`.
    fn exports(src: &str) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        for line in src.lines() {
            let l = line.trim_start();
            for kw in ["export function ", "export const "] {
                if let Some(rest) = l.strip_prefix(kw) {
                    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    out.insert(name);
                }
            }
        }
        out
    }

    #[test]
    fn reference_matches_module() {
        let declared = exports(super::API);
        let defined = exports(include_str!("../js/reel.ts"));
        assert_eq!(declared, defined, "reel.d.ts and reel.ts export different names");
    }

    #[test]
    fn module_strips() {
        crate::script::strip("reel.ts", include_str!("../js/reel.ts")).unwrap();
    }
}
