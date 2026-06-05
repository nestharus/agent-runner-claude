// declared_role: orchestration, parser, validator, mapper

use crate::envelope::error::ProviderFailure;

pub fn subcommand_from_args<I>(args: I) -> Result<String, ProviderFailure>
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let tokens = argv_tokens(args);
    validate_subcommand_tokens(&tokens)
}

fn argv_tokens<I>(args: I) -> Vec<String>
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    args.into_iter().map(Into::into).skip(1).collect()
}

fn validate_subcommand_tokens(tokens: &[String]) -> Result<String, ProviderFailure> {
    match tokens {
        [] => Err(ProviderFailure::unsupported(
            "missing_subcommand",
            "exactly one provider subcommand is required",
        )),
        [subcommand] => Ok(subcommand.clone()),
        _ => Err(ProviderFailure::unsupported(
            "extra_argv",
            "request data must not be supplied through argv",
        )),
    }
}
