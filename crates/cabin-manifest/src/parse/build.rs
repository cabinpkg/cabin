use crate::error::ManifestError;

/// Extract a `[build] compiler-wrapper = "..."` declaration.
pub(super) fn compiler_wrapper_request_from_raw(
    raw: Option<crate::raw::RawBuild>,
) -> Result<Option<cabin_core::CompilerWrapperRequest>, ManifestError> {
    let Some(value) = raw.and_then(|build| build.compiler_wrapper) else {
        return Ok(None);
    };
    let request = cabin_core::CompilerWrapperRequest::parse(&value).map_err(|source| {
        ManifestError::InvalidCompilerWrapper {
            section: "[build]".to_owned(),
            source,
        }
    })?;
    Ok(Some(request))
}
