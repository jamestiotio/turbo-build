use std::fmt::Write;

use anyhow::Result;
use turbo_tasks::{Value, ValueToString, Vc};
use turbo_tasks_fs::FileSystemPath;
use turbo_tasks_hash::{encode_hex, hash_xxh3_hash64, DeterministicHash, Xxh3Hash64Hasher};

use crate::resolve::ModulePart;

#[turbo_tasks::value(serialization = "auto_for_input")]
#[derive(Clone, Debug, PartialOrd, Ord, Hash)]
pub struct AssetIdent {
    /// The primary path of the asset
    pub path: Vc<FileSystemPath>,
    /// The query string of the asset (e.g. `?foo=bar`)
    pub query: Vc<String>,
    /// The fragment of the asset (e.g. `#foo`)
    pub fragment: Option<Vc<String>>,
    /// The assets that are nested in this asset
    pub assets: Vec<(Vc<String>, Vc<AssetIdent>)>,
    /// The modifiers of this asset (e.g. `client chunks`)
    pub modifiers: Vec<Vc<String>>,
    /// The part of the asset that is a (ECMAScript) module
    pub part: Option<Vc<ModulePart>>,
    /// The asset layer the asset was created from.
    pub layer: Option<Vc<String>>,
}

impl AssetIdent {
    pub fn add_modifier(&mut self, modifier: Vc<String>) {
        self.modifiers.push(modifier);
    }

    pub fn add_asset(&mut self, key: Vc<String>, asset: Vc<AssetIdent>) {
        self.assets.push((key, asset));
    }

    pub async fn rename_as_ref(&mut self, pattern: &str) -> Result<()> {
        let root = self.path.root();
        let path = self.path.await?;
        self.path = root
            .join(pattern.replace('*', &path.path))
            .resolve()
            .await?;
        Ok(())
    }
}

#[turbo_tasks::value_impl]
impl ValueToString for AssetIdent {
    #[turbo_tasks::function]
    async fn to_string(&self) -> Result<Vc<String>> {
        let mut s = self.path.to_string().await?.clone_value();

        let query = self.query.await?;
        if !query.is_empty() {
            write!(s, "?{}", &*query)?;
        }

        if let Some(fragment) = &self.fragment {
            write!(s, "#{}", fragment.await?)?;
        }

        for (key, asset) in &self.assets {
            write!(s, "/({})/{}", key.await?, asset.to_string().await?)?;
        }

        if let Some(layer) = &self.layer {
            write!(s, " [{}]", layer.await?)?;
        }

        if !self.modifiers.is_empty() {
            s.push_str(" (");

            for (i, modifier) in self.modifiers.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }

                s.push_str(&modifier.await?);
            }

            s.push(')');
        }

        if let Some(part) = self.part {
            write!(s, " {{{}}}", part.to_string().await?)?;
        }

        Ok(Vc::cell(s))
    }
}

#[turbo_tasks::value_impl]
impl AssetIdent {
    #[turbo_tasks::function]
    pub fn new(ident: Value<AssetIdent>) -> Vc<Self> {
        ident.into_value().cell()
    }

    /// Creates an [AssetIdent] from a [Vc<FileSystemPath>]
    #[turbo_tasks::function]
    pub fn from_path(path: Vc<FileSystemPath>) -> Vc<Self> {
        Self::new(Value::new(AssetIdent {
            path,
            query: Vc::<String>::default(),
            fragment: None,
            assets: Vec::new(),
            modifiers: Vec::new(),
            part: None,
            layer: None,
        }))
    }

    #[turbo_tasks::function]
    pub fn with_query(&self, query: Vc<String>) -> Vc<Self> {
        let mut this = self.clone();
        this.query = query;
        Self::new(Value::new(this))
    }

    #[turbo_tasks::function]
    pub fn with_modifier(&self, modifier: Vc<String>) -> Vc<Self> {
        let mut this = self.clone();
        this.add_modifier(modifier);
        Self::new(Value::new(this))
    }

    #[turbo_tasks::function]
    pub fn with_part(&self, part: Vc<ModulePart>) -> Vc<Self> {
        let mut this = self.clone();
        this.part = Some(part);
        Self::new(Value::new(this))
    }

    #[turbo_tasks::function]
    pub fn with_path(&self, path: Vc<FileSystemPath>) -> Vc<Self> {
        let mut this = self.clone();
        this.path = path;
        Self::new(Value::new(this))
    }

    #[turbo_tasks::function]
    pub fn with_layer(&self, layer: Vc<String>) -> Vc<Self> {
        let mut this = self.clone();
        this.layer = Some(layer);
        Self::new(Value::new(this))
    }

    #[turbo_tasks::function]
    pub async fn rename_as(&self, pattern: String) -> Result<Vc<Self>> {
        let mut this = self.clone();
        this.rename_as_ref(&pattern).await?;
        Ok(Self::new(Value::new(this)))
    }

    #[turbo_tasks::function]
    pub fn path(&self) -> Vc<FileSystemPath> {
        self.path
    }

    #[turbo_tasks::function]
    pub fn query(&self) -> Vc<String> {
        self.query
    }

    /// Computes a unique output asset name for the given asset identifier.
    /// TODO(alexkirsz) This is `turbopack-dev` specific, as `turbopack-build`
    /// would use a content hash instead. But for now both are using the same
    /// name generation logic.
    #[turbo_tasks::function]
    pub async fn output_name(
        &self,
        context_path: Vc<FileSystemPath>,
        expected_extension: String,
    ) -> Result<Vc<String>> {
        // For clippy -- This explicit deref is necessary
        let path = &*self.path.await?;
        let mut name = if let Some(inner) = context_path.await?.get_path_to(path) {
            clean_separators(inner)
        } else {
            clean_separators(&self.path.to_string().await?)
        };
        let removed_extension = name.ends_with(&expected_extension);
        if removed_extension {
            name.truncate(name.len() - expected_extension.len());
        }
        // This step ensures that leading dots are not preserved in file names. This is
        // important as some file servers do not serve files with leading dots (e.g.
        // Next.js).
        let mut name = clean_additional_extensions(&name);

        let default_modifier = match expected_extension.as_str() {
            ".js" => Some("ecmascript"),
            ".css" => Some("css"),
            _ => None,
        };

        let mut hasher = Xxh3Hash64Hasher::new();
        let mut has_hash = false;
        let AssetIdent {
            path: _,
            query,
            fragment,
            assets,
            modifiers,
            part,
            layer,
        } = self;
        let query = query.await?;
        if !query.is_empty() {
            0_u8.deterministic_hash(&mut hasher);
            query.deterministic_hash(&mut hasher);
            has_hash = true;
        }
        if let Some(fragment) = fragment {
            1_u8.deterministic_hash(&mut hasher);
            fragment.await?.deterministic_hash(&mut hasher);
            has_hash = true;
        }
        for (key, ident) in assets.iter() {
            2_u8.deterministic_hash(&mut hasher);
            key.await?.deterministic_hash(&mut hasher);
            ident.to_string().await?.deterministic_hash(&mut hasher);
            has_hash = true;
        }
        for modifier in modifiers.iter() {
            let modifier = modifier.await?;
            if let Some(default_modifier) = default_modifier {
                if *modifier == default_modifier {
                    continue;
                }
            }
            3_u8.deterministic_hash(&mut hasher);
            modifier.deterministic_hash(&mut hasher);
            has_hash = true;
        }
        if let Some(part) = part {
            4_u8.deterministic_hash(&mut hasher);
            match &*part.await? {
                ModulePart::Evaluation => {
                    1_u8.deterministic_hash(&mut hasher);
                }
                ModulePart::Export(export) => {
                    2_u8.deterministic_hash(&mut hasher);
                    export.await?.deterministic_hash(&mut hasher);
                }
                ModulePart::RenamedExport {
                    original_export,
                    export,
                } => {
                    3_u8.deterministic_hash(&mut hasher);
                    original_export.await?.deterministic_hash(&mut hasher);
                    export.await?.deterministic_hash(&mut hasher);
                }
                ModulePart::RenamedNamespace { export } => {
                    4_u8.deterministic_hash(&mut hasher);
                    export.await?.deterministic_hash(&mut hasher);
                }
                ModulePart::Internal(id) => {
                    5_u8.deterministic_hash(&mut hasher);
                    id.deterministic_hash(&mut hasher);
                }
                ModulePart::Locals => {
                    6_u8.deterministic_hash(&mut hasher);
                }
                ModulePart::Exports => {
                    7_u8.deterministic_hash(&mut hasher);
                }
                ModulePart::Facade => {
                    8_u8.deterministic_hash(&mut hasher);
                }
            }

            has_hash = true;
        }
        if let Some(layer) = layer {
            1_u8.deterministic_hash(&mut hasher);
            layer.await?.deterministic_hash(&mut hasher);
            has_hash = true;
        }

        if has_hash {
            let hash = encode_hex(hasher.finish());
            let truncated_hash = &hash[..6];
            write!(name, "_{}", truncated_hash)?;
        }

        // Location in "path" where hashed and named parts are split.
        // Everything before i is hashed and after i named.
        let mut i = 0;
        static NODE_MODULES: &str = "_node_modules_";
        if let Some(j) = name.rfind(NODE_MODULES) {
            i = j + NODE_MODULES.len();
        }
        const MAX_FILENAME: usize = 80;
        if name.len() - i > MAX_FILENAME {
            i = name.len() - MAX_FILENAME;
            if let Some(j) = name[i..].find('_') {
                if j < 20 {
                    i += j + 1;
                }
            }
        }
        if i > 0 {
            let hash = encode_hex(hash_xxh3_hash64(name[..i].as_bytes()));
            let truncated_hash = &hash[..5];
            name = format!("{}_{}", truncated_hash, &name[i..]);
        }
        // We need to make sure that `.json` and `.json.js` doesn't end up with the same
        // name. So when we add an extra extension when want to mark that with a "._"
        // suffix.
        if !removed_extension {
            name += "._";
        }
        name += &expected_extension;
        Ok(Vc::cell(name))
    }
}

fn clean_separators(s: &str) -> String {
    s.replace('/', "_")
}

fn clean_additional_extensions(s: &str) -> String {
    s.replace('.', "_")
}
