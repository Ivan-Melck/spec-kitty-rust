//! Native error interoperability controls, not extra Source execution pairs.
use std::error::Error;

use sk_doctrine::{
    ActivationContextError, ConstructorAbort, EffectiveSetError, GateFailure,
    NativeOfferingFailure, OfferingLoadError, WarningLog,
};
use sk_kernel::python_stream::StreamError;

fn native() -> NativeOfferingFailure {
    NativeOfferingFailure::ActivationWarningDelivery {
        cause: Box::new(StreamError::UnsupportedEncoding {
            name: "cp1252".to_owned(),
            detail: "Source supports this codec; Native does not yet".to_owned(),
        }),
        log: Box::new(WarningLog::default()),
    }
}

#[test]
fn standard_error_traversal_reaches_native_stream_cause() {
    let errors: Vec<Box<dyn Error>> = vec![
        Box::new(GateFailure::Native(native())),
        Box::new(OfferingLoadError::from_native(native())),
        Box::new(ActivationContextError::Native(native())),
        Box::new(EffectiveSetError::Native(native())),
    ];
    for error in errors {
        let cause = error.source().expect("wrapper must retain typed source");
        assert!(cause.downcast_ref::<NativeOfferingFailure>().is_some());
        let stream = cause.source().expect("native failure retains stream cause");
        assert!(matches!(
            stream.downcast_ref::<StreamError>(),
            Some(StreamError::UnsupportedEncoding { name, .. }) if name == "cp1252"
        ));
    }
}

#[test]
fn standard_error_traversal_keeps_constructor_abort_distinct() {
    let abort = ConstructorAbort {
        python: "ValueError",
        message: "constructor control".to_owned(),
    };
    let errors: Vec<Box<dyn Error>> = vec![
        Box::new(GateFailure::Constructor(abort.clone())),
        Box::new(OfferingLoadError::from_abort(abort.clone())),
        Box::new(EffectiveSetError::Fatal(abort)),
    ];
    for error in errors {
        let cause = error.source().expect("wrapper retains the Source abort");
        assert_eq!(cause.to_string(), "ValueError: constructor control");
        assert!(cause.source().is_none(), "constructor is the actual leaf");
        assert!(cause.downcast_ref::<NativeOfferingFailure>().is_none());
    }
}

#[test]
fn native_codec_gap_does_not_claim_a_python_startup_fatal() {
    let error = StreamError::UnsupportedEncoding {
        name: "cp1252".to_owned(),
        detail: "not yet implemented".to_owned(),
    };
    assert!(error.is_native_capability_gap());
    let text = error.to_string();
    assert!(!text.contains("Fatal Python error"), "{text}");
    assert!(text.contains("native") || text.contains("Native"), "{text}");
    assert!(text.contains("cp1252"));
}
