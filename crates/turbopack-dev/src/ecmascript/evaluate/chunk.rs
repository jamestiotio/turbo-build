use std::io::Write;

use anyhow::{bail, Result};
use indoc::writedoc;
use serde::Serialize;
use turbo_tasks::{ReadRef, TryJoinIterExt, Value, ValueToString, Vc};
use turbo_tasks_fs::File;
use turbopack_core::{
    asset::{Asset, AssetContent},
    chunk::{
        ChunkData, ChunkItemExt, ChunkableModule, ChunkingContext, ChunksData, EvaluatableAssets,
        ModuleId,
    },
    code_builder::{Code, CodeBuilder},
    ident::AssetIdent,
    module::Module,
    output::{OutputAsset, OutputAssets},
    source_map::{GenerateSourceMap, OptionSourceMap, SourceMapAsset},
};
use turbopack_ecmascript::{
    chunk::{EcmascriptChunkData, EcmascriptChunkPlaceable},
    utils::StringifyJs,
};
use turbopack_ecmascript_runtime::RuntimeType;

use crate::DevChunkingContext;

/// An Ecmascript chunk that:
/// * Contains the Turbopack dev runtime code; and
/// * Evaluates a list of runtime entries.
#[turbo_tasks::value(shared)]
pub(crate) struct EcmascriptDevEvaluateChunk {
    chunking_context: Vc<DevChunkingContext>,
    ident: Vc<AssetIdent>,
    other_chunks: Vc<OutputAssets>,
    evaluatable_assets: Vc<EvaluatableAssets>,
}

#[turbo_tasks::value_impl]
impl EcmascriptDevEvaluateChunk {
    /// Creates a new [`Vc<EcmascriptDevEvaluateChunk>`].
    #[turbo_tasks::function]
    pub fn new(
        chunking_context: Vc<DevChunkingContext>,
        ident: Vc<AssetIdent>,
        other_chunks: Vc<OutputAssets>,
        evaluatable_assets: Vc<EvaluatableAssets>,
    ) -> Vc<Self> {
        EcmascriptDevEvaluateChunk {
            chunking_context,
            ident,
            other_chunks,
            evaluatable_assets,
        }
        .cell()
    }

    #[turbo_tasks::function]
    async fn chunks_data(self: Vc<Self>) -> Result<Vc<ChunksData>> {
        let this = self.await?;
        Ok(ChunkData::from_assets(
            this.chunking_context.output_root(),
            this.other_chunks,
        ))
    }

    #[turbo_tasks::function]
    async fn code(self: Vc<Self>) -> Result<Vc<Code>> {
        let this = self.await?;
        let chunking_context = this.chunking_context.await?;
        let environment = this.chunking_context.environment();

        let output_root = this.chunking_context.output_root().await?;
        let chunk_path = self.ident().path().await?;
        let chunk_public_path = if let Some(path) = output_root.get_path_to(&chunk_path) {
            path
        } else {
            bail!(
                "chunk path {} is not in output root {}",
                chunk_path.to_string(),
                output_root.to_string()
            );
        };

        let other_chunks_data = self.chunks_data().await?;
        let other_chunks_data = other_chunks_data.iter().try_join().await?;
        let other_chunks_data: Vec<_> = other_chunks_data
            .iter()
            .map(|chunk_data| EcmascriptChunkData::new(chunk_data))
            .collect();

        let runtime_module_ids = this
            .evaluatable_assets
            .await?
            .iter()
            .map({
                let chunking_context = this.chunking_context;
                move |entry| async move {
                    if let Some(placeable) =
                        Vc::try_resolve_sidecast::<Box<dyn EcmascriptChunkPlaceable>>(*entry)
                            .await?
                    {
                        Ok(Some(
                            placeable
                                .as_chunk_item(Vc::upcast(chunking_context))
                                .id()
                                .await?,
                        ))
                    } else {
                        Ok(None)
                    }
                }
            })
            .try_join()
            .await?
            .into_iter()
            .flatten()
            .collect();

        let params = EcmascriptDevChunkRuntimeParams {
            other_chunks: &other_chunks_data,
            runtime_module_ids,
        };

        let mut code = CodeBuilder::default();

        // We still use the `TURBOPACK` global variable to store the chunk here,
        // as there may be another runtime already loaded in the page.
        // This is the case in integration tests.
        writedoc!(
            code,
            r#"
                (globalThis.TURBOPACK = globalThis.TURBOPACK || []).push([
                    {},
                    {{}},
                    {}
                ]);
            "#,
            StringifyJs(&chunk_public_path),
            StringifyJs(&params),
        )?;

        match chunking_context.runtime_type() {
            RuntimeType::Default => {
                let runtime_code = turbopack_ecmascript_runtime::get_dev_runtime_code(
                    environment,
                    chunking_context.chunk_base_path(),
                    Vc::cell(output_root.to_string()),
                );
                code.push_code(&*runtime_code.await?);
            }
            #[cfg(feature = "test")]
            RuntimeType::Dummy => {
                let runtime_code = turbopack_ecmascript_runtime::get_dummy_runtime_code();
                code.push_code(&runtime_code);
            }
        }

        if code.has_source_map() {
            let filename = chunk_path.file_name();
            write!(code, "\n\n//# sourceMappingURL={}.map", filename)?;
        }

        Ok(Code::cell(code.build()))
    }
}

#[turbo_tasks::value_impl]
impl ValueToString for EcmascriptDevEvaluateChunk {
    #[turbo_tasks::function]
    async fn to_string(&self) -> Result<Vc<String>> {
        Ok(Vc::cell("Ecmascript Dev Evaluate Chunk".to_string()))
    }
}

#[turbo_tasks::function]
fn modifier() -> Vc<String> {
    Vc::cell("ecmascript dev evaluate chunk".to_string())
}

#[turbo_tasks::value_impl]
impl OutputAsset for EcmascriptDevEvaluateChunk {
    #[turbo_tasks::function]
    async fn ident(&self) -> Result<Vc<AssetIdent>> {
        let mut ident = self.ident.await?.clone_value();

        ident.add_modifier(modifier());

        let evaluatable_assets = self.evaluatable_assets.await?;
        ident.modifiers.extend(
            evaluatable_assets
                .iter()
                .map(|entry| entry.ident().to_string()),
        );

        for chunk in &*self.other_chunks.await? {
            ident.add_modifier(chunk.ident().to_string());
        }

        let ident = AssetIdent::new(Value::new(ident));
        Ok(AssetIdent::from_path(
            self.chunking_context.chunk_path(ident, ".js".to_string()),
        ))
    }

    #[turbo_tasks::function]
    async fn references(self: Vc<Self>) -> Result<Vc<OutputAssets>> {
        let this = self.await?;
        let mut references = Vec::new();

        let include_source_map = *this
            .chunking_context
            .reference_chunk_source_maps(Vc::upcast(self))
            .await?;

        if include_source_map {
            references.push(Vc::upcast(SourceMapAsset::new(Vc::upcast(self))));
        }

        for chunk_data in &*self.chunks_data().await? {
            references.extend(chunk_data.references().await?.iter().copied());
        }

        Ok(Vc::cell(references))
    }
}

#[turbo_tasks::value_impl]
impl Asset for EcmascriptDevEvaluateChunk {
    #[turbo_tasks::function]
    async fn content(self: Vc<Self>) -> Result<Vc<AssetContent>> {
        let code = self.code().await?;
        Ok(AssetContent::file(
            File::from(code.source_code().clone()).into(),
        ))
    }
}

#[turbo_tasks::value_impl]
impl GenerateSourceMap for EcmascriptDevEvaluateChunk {
    #[turbo_tasks::function]
    fn generate_source_map(self: Vc<Self>) -> Vc<OptionSourceMap> {
        self.code().generate_source_map()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EcmascriptDevChunkRuntimeParams<'a, T> {
    /// Other chunks in the chunk group this chunk belongs to, if any. Does not
    /// include the chunk itself.
    ///
    /// These chunks must be loaed before the runtime modules can be
    /// instantiated.
    other_chunks: &'a [T],
    /// List of module IDs that this chunk should instantiate when executed.
    runtime_module_ids: Vec<ReadRef<ModuleId>>,
}
