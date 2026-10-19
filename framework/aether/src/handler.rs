//! Handler dispatch for the typed extractor system.
//!
//! [`Handler`] is implemented for async closures of up to four arguments. Every argument
//! before the last must implement [`FromRequestParts`]; the last one implements
//! [`FromRequest`]. The existing `|req: RequestContext| async { .. }` shape keeps working
//! because `RequestContext` itself implements [`FromRequest`].
//!
//! Handwritten impls (rather than a blanket macro) keep the trait auditable: each arity
//! extracts its parts in declaration order and short-circuits with the rejection response on
//! the first failure.

use std::future::Future;
use std::sync::Arc;

use crate::app::{BoxFutureResponse, IntoResponse, RequestContext};
use crate::extract::{FromRequest, FromRequestParts};

/// An async handler callable with an extracted argument tuple `Args`.
pub trait Handler<Args>: Send + Sync + 'static {
    fn call(&self, ctx: RequestContext) -> BoxFutureResponse;
}

impl<F, Fut, R> Handler<()> for F
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + Send + 'static,
{
    fn call(&self, _ctx: RequestContext) -> BoxFutureResponse {
        let fut = self();
        Box::pin(async move { fut.await.into_response() })
    }
}

impl<F, Fut, R, E1> Handler<(E1,)> for F
where
    F: Fn(E1) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + Send + 'static,
    E1: FromRequest + Send,
{
    fn call(&self, ctx: RequestContext) -> BoxFutureResponse {
        match E1::from_request(ctx) {
            Ok(e1) => {
                let fut = self(e1);
                Box::pin(async move { fut.await.into_response() })
            }
            Err(response) => Box::pin(async move { response }),
        }
    }
}

impl<F, Fut, R, P1, E2> Handler<(P1, E2)> for F
where
    F: Fn(P1, E2) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + Send + 'static,
    P1: FromRequestParts + Send,
    E2: FromRequest + Send,
{
    fn call(&self, ctx: RequestContext) -> BoxFutureResponse {
        match P1::from_parts(&ctx) {
            Ok(p1) => match E2::from_request(ctx) {
                Ok(e2) => {
                    let fut = self(p1, e2);
                    Box::pin(async move { fut.await.into_response() })
                }
                Err(response) => Box::pin(async move { response }),
            },
            Err(response) => Box::pin(async move { response }),
        }
    }
}

impl<F, Fut, R, P1, P2, E3> Handler<(P1, P2, E3)> for F
where
    F: Fn(P1, P2, E3) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + Send + 'static,
    P1: FromRequestParts + Send,
    P2: FromRequestParts + Send,
    E3: FromRequest + Send,
{
    fn call(&self, ctx: RequestContext) -> BoxFutureResponse {
        match P1::from_parts(&ctx) {
            Ok(p1) => match P2::from_parts(&ctx) {
                Ok(p2) => match E3::from_request(ctx) {
                    Ok(e3) => {
                        let fut = self(p1, p2, e3);
                        Box::pin(async move { fut.await.into_response() })
                    }
                    Err(response) => Box::pin(async move { response }),
                },
                Err(response) => Box::pin(async move { response }),
            },
            Err(response) => Box::pin(async move { response }),
        }
    }
}

impl<F, Fut, R, P1, P2, P3, E4> Handler<(P1, P2, P3, E4)> for F
where
    F: Fn(P1, P2, P3, E4) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + Send + 'static,
    P1: FromRequestParts + Send,
    P2: FromRequestParts + Send,
    P3: FromRequestParts + Send,
    E4: FromRequest + Send,
{
    fn call(&self, ctx: RequestContext) -> BoxFutureResponse {
        match P1::from_parts(&ctx) {
            Ok(p1) => match P2::from_parts(&ctx) {
                Ok(p2) => match P3::from_parts(&ctx) {
                    Ok(p3) => match E4::from_request(ctx) {
                        Ok(e4) => {
                            let fut = self(p1, p2, p3, e4);
                            Box::pin(async move { fut.await.into_response() })
                        }
                        Err(response) => Box::pin(async move { response }),
                    },
                    Err(response) => Box::pin(async move { response }),
                },
                Err(response) => Box::pin(async move { response }),
            },
            Err(response) => Box::pin(async move { response }),
        }
    }
}

/// Shared registration helper: type-erase a [`Handler`] into the app's stored dispatch
/// signature.
pub(crate) fn erase<Args, F>(handler: F) -> crate::app::AsyncHandlerFn
where
    F: Handler<Args>,
    Args: 'static,
{
    let handler = Arc::new(handler);
    Arc::new(move |ctx| {
        let h = Arc::clone(&handler);
        h.call(ctx)
    })
}
