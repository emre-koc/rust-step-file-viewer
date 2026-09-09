//! Exporter conventions are hints, not a guaranteed semantic up direction in STEP.
use step_p21::Header;
use step_render::UpAxis;

pub fn suggested_up_axis(header: &Header) -> UpAxis {
    let system = header.originating_system.to_ascii_lowercase();
    let preprocessor = header.preprocessor_version.to_ascii_lowercase();
    if system.contains("solidworks") || preprocessor.starts_with("swstep") {
        UpAxis::Y
    } else {
        UpAxis::Z
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exporter_hints_do_not_depend_on_file_names_or_model_dimensions() {
        for (system, preprocessor, expected) in [
            ("SolidWorks 2012", "SwSTEP 2.0", UpAxis::Y),
            ("SOLIDWORKS 2024", "", UpAxis::Y),
            ("", "SwSTEP 2.0", UpAxis::Y),
            ("Open CASCADE 7.9", "", UpAxis::Z),
            ("Creo Parametric", "", UpAxis::Z),
            ("", "", UpAxis::Z),
        ] {
            let header = Header { originating_system: system.into(), preprocessor_version: preprocessor.into(), ..Default::default() };
            assert_eq!(suggested_up_axis(&header), expected);
        }
    }
}
