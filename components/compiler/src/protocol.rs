use crate::api;
use wavelet::compiler as core;

pub fn request(value: api::Request) -> core::Request {
    core::Request {
        sources: value
            .sources
            .into_iter()
            .map(|source| core::SourceFile {
                name: source.name,
                contents: source.contents,
            })
            .collect(),
        dependencies: value
            .dependencies
            .into_iter()
            .map(|file| core::DependencyFile {
                name: file.name,
                contents: file.contents,
            })
            .collect(),
        entry: value.entry,
        mode: match value.mode {
            api::Mode::Project => core::Mode::Project,
            api::Mode::Snippet => core::Mode::Snippet,
            api::Mode::Repl => core::Mode::Repl,
        },
    }
}

pub fn step(value: core::Step) -> api::Step {
    match value {
        core::Step::NeedsMacros(requests) => api::Step::NeedsMacros(
            requests
                .into_iter()
                .map(|request| api::MacroRequest {
                    key: request.key,
                    source: match request.source {
                        core::MacroSource::Component(bytes) => api::MacroSource::Component(bytes),
                        core::MacroSource::Imported(import) => {
                            api::MacroSource::Imported(api::MacroImport {
                                package_id: import.package_id,
                                source_path: import.source_path,
                            })
                        }
                    },
                })
                .collect(),
        ),
        core::Step::Complete(compilation) => api::Step::Complete(api::Compilation {
            artifacts: compilation.artifacts.into_iter().map(artifact).collect(),
            entry: compilation.entry,
            invocation: compilation.invocation.map(|invocation| api::Invocation {
                interface_name: invocation.interface_name,
                function_name: invocation.function_name,
            }),
        }),
    }
}

fn artifact(value: core::Artifact) -> api::Artifact {
    api::Artifact {
        name: value.name,
        package_id: value.package_id,
        bytes: value.bytes,
        kind: match value.kind {
            core::ArtifactKind::Component => api::ArtifactKind::Component,
            core::ArtifactKind::MacroComponent => api::ArtifactKind::MacroComponent,
            core::ArtifactKind::Application => api::ArtifactKind::Application,
        },
    }
}

pub fn diagnostic(value: core::Diagnostic) -> api::Diagnostic {
    api::Diagnostic {
        phase: match value.phase {
            core::Phase::Input => api::Phase::Input,
            core::Phase::Read => api::Phase::Read,
            core::Phase::Expand => api::Phase::Expand,
            core::Phase::Check => api::Phase::Check,
            core::Phase::Wit => api::Phase::Wit,
            core::Phase::Emit => api::Phase::Emit,
            core::Phase::Compose => api::Phase::Compose,
        },
        source: value.source,
        span: value.span.map(|span| api::SourceSpan {
            start: span.start,
            end: span.end,
        }),
        message: value.message,
    }
}
