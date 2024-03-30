use futures_util::FutureExt;
use crate::cells::{CodeCell, SupportedLanguage};
use crate::execution::primitives::operation::{InputItemConfiguation, InputSignature, InputType, OperationNode, OutputItemConfiguation, OutputSignature};

/// Code cells allow notebooks to evaluate source code in a variety of languages.
#[tracing::instrument]
pub fn code_cell(cell: &CodeCell) -> OperationNode {
    match cell.language {
        SupportedLanguage::PyO3 => {
            // TODO: all callable functions should be exposed as their own OperationNodes
            //       these can be depended on throughout the graph as their own cells.
            let paths =
                chidori_static_analysis::language::python::parse::extract_dependencies_python(
                    &cell.source_code,
                );
            let report = chidori_static_analysis::language::python::parse::build_report(&paths);

            let mut input_signature = InputSignature::new();
            for (key, value) in &report.cell_depended_values {
                input_signature.globals.insert(
                    key.clone(),
                    InputItemConfiguation {
                        ty: Some(InputType::String),
                        default: None,
                    },
                );
            }

            let mut output_signature = OutputSignature::new();
            for (key, value) in &report.cell_exposed_values {
                output_signature.globals.insert(
                    key.clone(),
                    OutputItemConfiguation {
                        ty: Some(InputType::String),
                    },
                );
            }

            for (key, value) in &report.triggerable_functions {
                output_signature.functions.insert(
                    key.clone(),
                    OutputItemConfiguation {
                        ty: Some(InputType::Function),
                    },
                );
            }

            let cell = cell.clone();
            OperationNode::new(
                cell.name.clone(),
                input_signature,
                output_signature,
                Box::new(move |x, _| {
                    let cell = cell.clone();
                    // TODO: this needs to handle stdout and errors
                    async move {
                        crate::library::std::code::runtime_pyo3::source_code_run_python(
                            &cell.source_code,
                            &x,
                            &cell.function_invocation,
                        ).await
                            .unwrap()
                            .0
                    }.boxed()
                }),
            )
        }
        SupportedLanguage::Starlark => OperationNode::new(
            cell.name.clone(),
            InputSignature::new(),
            OutputSignature::new(),
            Box::new(|x, _| async move { x}.boxed()),
        ),
        SupportedLanguage::Deno => {
            let paths =
                chidori_static_analysis::language::javascript::parse::extract_dependencies_js(
                    &cell.source_code,
                );
            let report = chidori_static_analysis::language::javascript::parse::build_report(&paths);

            let mut input_signature = InputSignature::new();
            for (key, value) in &report.cell_depended_values {
                input_signature.globals.insert(
                    key.clone(),
                    InputItemConfiguation {
                        ty: Some(InputType::String),
                        default: None,
                    },
                );
            }

            let mut output_signature = OutputSignature::new();
            for (key, value) in &report.cell_exposed_values {
                output_signature.globals.insert(
                    key.clone(),
                    OutputItemConfiguation {
                        ty: Some(InputType::String),
                    },
                );
            }

            for (key, value) in &report.triggerable_functions {
                output_signature.functions.insert(
                    key.clone(),
                    OutputItemConfiguation {
                        ty: Some(InputType::Function),
                    },
                );
            }

            let cell = cell.clone();
            OperationNode::new(
                cell.name.clone(),
                input_signature,
                output_signature,
                Box::new(move |x, _| {
                    // TODO: this needs to handle stdout and errors
                    // TODO: this code is crazy, there needs to be a better way to handle this.
                    //       the issue is that source_code_run_deno uses awaits that return values that are not send
                    let cell = cell.clone();
                    async move {
                        let result = tokio::task::spawn_blocking(move || {
                            let runtime = tokio::runtime::Runtime::new().unwrap();
                            let result = runtime.block_on(crate::library::std::code::runtime_deno::source_code_run_deno(
                                &cell.source_code,
                                &x,
                                &cell.function_invocation,
                            ));
                            match result {
                                Ok(v) => v.0,
                                Err(e) => panic!("{:?}", e),
                            }
                        }).await.unwrap();
                        result
                    }.boxed()
                }),
            )
        }
    }
}


#[cfg(test)]
mod test {
    #[tokio::test]
    async fn test_code_cell() {


    }
}