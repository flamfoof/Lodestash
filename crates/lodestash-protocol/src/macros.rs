/// Declares the operation catalog. One entry per operation:
///
/// ```text
/// /* name */ search_assets:
///     GET "/v1/search" (SearchArgs) -> Value, Agent, Read,
///     "The summary — an Agent operation's MCP tool description.";
/// ```
///
/// From the one list it generates [`OPERATIONS`](crate::OPERATIONS), the
/// [`Operations`](crate::Operations) trait (one method per entry, so an implementor that misses an
/// operation does not compile) and [`dispatch`](crate::dispatch), which runs any entry by name on a
/// JSON input. In tests it also generates `Echo`, an implementation that answers every operation
/// with the input it decoded.
macro_rules! operations {
    (@method GET) => { HttpMethod::Get };
    (@method POST) => { HttpMethod::Post };
    (@method PUT) => { HttpMethod::Put };
    (@method DELETE) => { HttpMethod::Delete };

    ($(
        $name:ident : $method:ident $path:literal ($input:ty) -> $output:ty,
            $audience:ident, $effect:ident, $summary:literal;
    )*) => {
        /// Every operation, in declaration order.
        pub static OPERATIONS: &[OpSpec] = &[$(
            OpSpec {
                name: stringify!($name),
                method: operations!(@method $method),
                path: $path,
                audience: Audience::$audience,
                effect: Effect::$effect,
                summary: $summary,
                input_schema: schema_for::<$input>,
                output_schema: schema_for::<$output>,
                canonical_input: canonical_input::<$input>,
            },
        )*];

        /// The operations, one method each. The engine implements this (v2 plan P3); the HTTP
        /// adapter reaches it through [`dispatch`].
        ///
        /// Methods are synchronous: the engine's work is blocking (its writer, readers and lanes),
        /// and an async adapter calls them from its blocking pool.
        pub trait Operations {
            $(
                #[doc = $summary]
                fn $name(&self, caller: &Caller, input: $input) -> Result<$output, ApiError>;
            )*
        }

        /// Run the operation called `name` on `ops`: decode `input` into its input type (a null
        /// field is an absent one), call it, encode its output. An unknown name is `not_found`; an
        /// input the type refuses is `invalid`.
        ///
        /// Audience and caller checks are the adapter's, before it dispatches.
        pub fn dispatch<O: Operations + ?Sized>(
            ops: &O,
            name: &str,
            caller: &Caller,
            input: Value,
        ) -> Result<Value, ApiError> {
            match name {
                $(stringify!($name) => {
                    let input = decode_input::<$input>(input).map_err(|e| of_op(name, e))?;
                    encode_output(ops.$name(caller, input)?)
                })*
                other => Err(ApiError::new(
                    codes::NOT_FOUND,
                    format!("no operation is named {other:?}"),
                )),
            }
        }

        /// Answers every operation with `echo` carrying the input it decoded.
        #[cfg(test)]
        pub(crate) struct Echo;

        #[cfg(test)]
        impl Operations for Echo {
            $(
                fn $name(&self, _caller: &Caller, input: $input) -> Result<$output, ApiError> {
                    Err(ApiError::new("echo", stringify!($name))
                        .with_detail(serde_json::to_value(&input).unwrap()))
                }
            )*
        }
    };
}
