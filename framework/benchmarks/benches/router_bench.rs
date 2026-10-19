use aether_http::method::Method;
use aether_router::trie::RadixRouter;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_radix_router(c: &mut Criterion) {
    let mut router = RadixRouter::new();
    router.add_route(Method::Get, "/", 1);
    router.add_route(Method::Get, "/users", 2);
    router.add_route(Method::Get, "/users/{id}", 3);
    router.add_route(Method::Post, "/users/{id}/posts/{post_id}", 4);
    router.add_route(Method::Get, "/api/v1/health", 5);

    c.bench_function("router_lookup_static", |b| {
        b.iter(|| {
            black_box(router.find(Method::Get, black_box("/api/v1/health")));
        })
    });

    c.bench_function("router_lookup_two_params", |b| {
        b.iter(|| {
            black_box(router.find(Method::Post, black_box("/users/123/posts/456")));
        })
    });
}

criterion_group!(benches, bench_radix_router);
criterion_main!(benches);
