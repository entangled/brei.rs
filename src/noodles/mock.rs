// ~/~ begin <<docs/src/engine.md#src/noodles/mock.rs>>[init]
use std::collections::HashMap;
use std::sync::{Arc, Barrier};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use super::task::{Task, Promise, Response, Error, InternalError, CancelToken};
use super::kernel::Kernel;
use super::single_worker::SingleWorker;
use super::worker_pool::WorkerPool;

type Func = Arc<dyn Fn(&[i64], &CancelToken) -> Response<Mock> + Send + Sync>;

struct Mock {
    key: u64,
    deps: Vec<u64>,
    args: HashMap<u64, i64>,
    memoize: bool,
    func: Func,
}

impl Mock {
    fn new(key: u64, deps: &[u64], func: impl Fn(&[i64]) -> Response<Mock> + Send + Sync + 'static) -> Self {
        Self::cancellable(key, deps, move |x, _| func(x))
    }

    fn cancellable(key: u64, deps: &[u64], func: impl Fn(&[i64], &CancelToken) -> Response<Mock> + Send + Sync + 'static) -> Self {
        Mock { key, deps: deps.to_vec(), args: HashMap::new(), memoize: false, func: Arc::new(func) }
    }

    fn memoized(mut self) -> Self {
        self.memoize = true;
        self
    }
}

impl Task for Mock {
    type Value = i64;
    type Key = u64;
    type Error = String;

    fn key(&self) -> u64 { self.key }

    fn dependencies(&self) -> impl Iterator<Item=u64> {
        self.deps.iter().copied()
    }

    fn memoize(&self) -> bool { self.memoize }

    fn run(self, cancel: &CancelToken) -> Response<Self> {
        let args: Vec<i64> = self.deps.iter().map(|k| self.args[k]).collect();
        (self.func)(&args, cancel)
    }

    fn assoc(&mut self, key: u64, value: i64) {
        self.args.insert(key, value);
    }

    fn ready(&self) -> bool {
        self.deps.iter().all(|k| self.args.contains_key(k))
    }
}

// ~/~ begin <<docs/src/engine.md#mock-helpers>>[init]
fn promise(root: u64, tasks: Vec<Mock>) -> Box<Promise<Mock>> {
    Box::new(Promise::new(root, tasks))
}

fn constant(key: u64, value: i64) -> Mock {
    Mock::new(key, &[], move |_| Response::Value(value))
}

fn binop(key: u64, a: u64, b: u64, f: fn(i64, i64) -> i64) -> Mock {
    Mock::new(key, &[a, b], move |x| Response::Value(f(x[0], x[1])))
}
// ~/~ end
// ~/~ begin <<docs/src/engine.md#mock-recursion>>[init]
fn factorial(key: u64, n: i64) -> Mock {
    Mock::new(key, &[], move |_| {
        if n <= 1 {
            return Response::Value(1);
        }
        Response::Continuation(promise(2, vec![
            factorial(1, n - 1),
            Mock::new(2, &[1], move |x| Response::Value(n * x[0])),
        ]))
    })
}
// ~/~ end
// ~/~ begin <<docs/src/engine.md#mock-recursion>>[1]
fn factorial_acc(key: u64, n: i64, acc: i64) -> Mock {
    Mock::new(key, &[], move |_| {
        if n <= 1 {
            return Response::Value(acc);
        }
        Response::Continuation(promise(1, vec![factorial_acc(1, n - 1, acc * n)]))
    })
}
// ~/~ end
// ~/~ begin <<docs/src/engine.md#mock-recursion>>[2]
fn fibonacci(key: u64, n: i64) -> Mock {
    Mock::new(key, &[], move |_| {
        if n < 2 {
            return Response::Value(n);
        }
        Response::Continuation(promise(3, vec![
            fibonacci(1, n - 1),
            fibonacci(2, n - 2),
            binop(3, 1, 2, |a, b| a + b),
        ]))
    })
}

fn sum_to(key: u64, n: i64) -> Mock {
    Mock::new(key, &[], move |_| {
        if n == 0 {
            return Response::Value(0);
        }
        Response::Continuation(promise(2, vec![
            sum_to(1, n - 1),
            Mock::new(2, &[1], move |x| Response::Value(n + x[0])),
        ]))
    })
}
// ~/~ end
// ~/~ begin <<docs/src/engine.md#mock-memo>>[init]
const SUM: u64 = u64::MAX;

fn fibonacci_memo(n: u64, runs: Arc<AtomicUsize>) -> Mock {
    Mock::new(n, &[], move |_| {
        runs.fetch_add(1, Ordering::Relaxed);
        if n < 2 {
            return Response::Value(n as i64);
        }
        Response::Continuation(promise(SUM, vec![
            fibonacci_memo(n - 1, runs.clone()),
            fibonacci_memo(n - 2, runs.clone()),
            binop(SUM, n - 1, n - 2, |a, b| a + b),
        ]))
    }).memoized()
}

fn sum_acc(key: u64, n: i64, acc: i64) -> Mock {
    Mock::new(key, &[], move |_| {
        if n == 0 {
            return Response::Value(acc);
        }
        Response::Continuation(promise(1, vec![sum_acc(1, n - 1, acc + n)]))
    })
}
// ~/~ end
// ~/~ begin <<docs/src/engine.md#mock-memo>>[1]
fn needs(key: u64, dep: u64) -> Mock {
    Mock::new(key, &[], move |_| Response::Continuation(promise(SUM, vec![
        needs(dep, key),
        Mock::new(SUM, &[dep], |x| Response::Value(x[0])),
    ]))).memoized()
}
// ~/~ end

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(p: Box<Promise<Mock>>) -> Result<i64, Error<Mock>> {
        let worker = SingleWorker::new();
        let mut kernel = Kernel::new();
        kernel.run(&worker, p)
    }

    // ~/~ begin <<docs/src/engine.md#mock-tests>>[init]
    #[test]
    fn diamond() {
        let p = promise(5, vec![
            constant(1, 2),
            constant(2, 3),
            binop(3, 1, 2, |a, b| a + b),
            binop(4, 1, 2, |a, b| a * b),
            binop(5, 3, 4, |a, b| a * b),
        ]);
        assert_eq!(eval(p).unwrap(), 30);
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[1]
    #[test]
    fn error() {
        let p = promise(3, vec![
            constant(1, 1),
            constant(2, 0),
            Mock::new(3, &[1, 2], |x| if x[1] == 0 {
                Response::Error("division by zero".into())
            } else {
                Response::Value(x[0] / x[1])
            }),
        ]);
        assert!(matches!(eval(p), Err(Error::Jobs(e)) if e == ["division by zero"]));
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[2]
    #[test]
    fn missing_dependency() {
        let p = promise(2, vec![binop(2, 1, 3, |a, b| a + b), constant(1, 1)]);
        assert!(matches!(eval(p), Err(Error::Internal(InternalError::NoResult))));
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[3]
    #[test]
    fn recursion() {
        for (n, f) in [(0, 1), (1, 1), (5, 120), (20, 2432902008176640000)] {
            assert_eq!(eval(promise(0, vec![factorial(0, n)])).unwrap(), f);
            assert_eq!(eval(promise(0, vec![factorial_acc(0, n, 1)])).unwrap(), f);
        }
        assert_eq!(eval(promise(0, vec![fibonacci(0, 20)])).unwrap(), 6765);
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[4]
    #[test]
    fn deep_recursion() {
        let n = 100_000;
        assert_eq!(eval(promise(0, vec![sum_to(0, n)])).unwrap(), n * (n + 1) / 2);
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[5]
    #[test]
    fn memoization() {
        let runs = Arc::new(AtomicUsize::new(0));
        let worker = SingleWorker::new();
        let mut kernel = Kernel::new();
        let fib50 = 12_586_269_025;

        assert_eq!(kernel.run(&worker, promise(50, vec![fibonacci_memo(50, runs.clone())])).unwrap(), fib50);
        assert_eq!(runs.load(Ordering::Relaxed), 51);
        assert_eq!(kernel.memo_count(), 51);

        assert_eq!(kernel.run(&worker, promise(50, vec![fibonacci_memo(50, runs.clone())])).unwrap(), fib50);
        assert_eq!(runs.load(Ordering::Relaxed), 51);
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[6]
    #[test]
    fn local_loop() {
        let worker = SingleWorker::new();
        let mut kernel = Kernel::new();
        let n = 1_000_000;
        assert_eq!(kernel.run(&worker, promise(0, vec![sum_acc(0, n, 0)])).unwrap(), n * (n + 1) / 2);
        assert_eq!(kernel.memo_count(), 0);
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[7]
    #[test]
    fn cycle() {
        let looping = Mock::new(1, &[], |_| Response::Continuation(promise(2, vec![
            Mock::new(1, &[], |_| Response::Value(0)).memoized(),
            Mock::new(2, &[1], |x| Response::Value(x[0])),
        ]))).memoized();
        assert!(matches!(eval(promise(1, vec![looping])), Err(Error::Cycle(c)) if c == [1]));
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[8]
    #[test]
    fn indirect_cycle() {
        assert!(matches!(eval(promise(1, vec![needs(1, 2)])), Err(Error::Cycle(c)) if c == [1, 2]));
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[9]
    #[test]
    fn local_cycle() {
        let looping = || vec![
            Mock::new(1, &[2], |x| Response::Value(x[0])),
            Mock::new(2, &[3], |x| Response::Value(x[0])),
            Mock::new(3, &[2], |x| Response::Value(x[0])).memoized(),
            Mock::new(4, &[4], |x| Response::Value(x[0])),
        ];
        let is_cycle = |c: &[u64]| c == [2, 3] || c == [3, 2] || c == [4];
        assert!(matches!(eval(promise(1, looping())), Err(Error::Cycle(c)) if is_cycle(&c)));

        let continued = Mock::new(0, &[], move |_| Response::Continuation(promise(1, looping())));
        assert!(matches!(eval_pool(promise(0, vec![continued])), Err(Error::Cycle(c)) if is_cycle(&c)));

        let add = |key, a, b| binop(key, a, b, |x, y| x + y);
        let diamond = vec![constant(1, 1), add(2, 1, 1), add(3, 1, 2), add(4, 2, 3)];
        assert_eq!(eval(promise(4, diamond)).unwrap(), 5);
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[10]
    fn eval_pool(p: Box<Promise<Mock>>) -> Result<i64, Error<Mock>> {
        let worker = WorkerPool::new(4);
        let mut kernel = Kernel::new();
        kernel.run(&worker, p)
    }

    #[test]
    fn pool_computations() {
        let p = promise(3, vec![constant(1, 2), constant(2, 3), binop(3, 1, 2, |a, b| a * b)]);
        assert_eq!(eval_pool(p).unwrap(), 6);
        assert_eq!(eval_pool(promise(0, vec![factorial(0, 20)])).unwrap(), 2432902008176640000);
        assert_eq!(eval_pool(promise(0, vec![factorial_acc(0, 20, 1)])).unwrap(), 2432902008176640000);
        assert_eq!(eval_pool(promise(0, vec![fibonacci(0, 20)])).unwrap(), 6765);
        assert_eq!(eval_pool(promise(0, vec![sum_to(0, 10_000)])).unwrap(), 50_005_000);
    }

    #[test]
    fn pool_memoization() {
        let runs = Arc::new(AtomicUsize::new(0));
        assert_eq!(eval_pool(promise(50, vec![fibonacci_memo(50, runs.clone())])).unwrap(), 12_586_269_025);
        assert_eq!(runs.load(Ordering::Relaxed), 51);
    }

    #[test]
    fn pool_error() {
        let p = promise(2, vec![
            Mock::new(1, &[], |_| Response::Error("failed".into())),
            Mock::new(2, &[1], |x| Response::Value(x[0])),
        ]);
        assert!(matches!(eval_pool(p), Err(Error::Jobs(e)) if e == ["failed"]));
        assert!(matches!(eval_pool(promise(2, vec![binop(2, 1, 3, |a, b| a + b)])), Err(Error::Internal(InternalError::NoResult))));
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[11]
    #[test]
    fn pool_parallel() {
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let mut tasks: Vec<Mock> = (1..=4).map(|k| {
            let barrier = barrier.clone();
            Mock::new(k, &[], move |_| {
                barrier.wait();
                Response::Value(k as i64)
            })
        }).collect();
        tasks.push(Mock::new(5, &[1, 2, 3, 4], |x| Response::Value(x.iter().sum())));
        assert_eq!(eval_pool(promise(5, tasks)).unwrap(), 10);
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[12]
    #[test]
    #[should_panic(expected = "boom")]
    fn pool_panic() {
        let _ = eval_pool(promise(1, vec![Mock::new(1, &[], |_| panic!("boom"))]));
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[13]
    fn failing_next_to_fibonacci() -> Box<Promise<Mock>> {
        promise(SUM, vec![
            Mock::new(100, &[], |_| Response::Error("failed".into())),
            fibonacci_memo(10, Arc::new(AtomicUsize::new(0))),
            binop(SUM, 100, 10, |a, b| a + b),
        ])
    }

    #[test]
    fn keep_going() {
        let worker = SingleWorker::new();
        let mut kernel = Kernel::new().keep_going();
        assert!(matches!(kernel.run(&worker, failing_next_to_fibonacci()), Err(Error::Jobs(e)) if e == ["failed"]));
        assert_eq!(kernel.memo_count(), 11);

        let mut kernel = Kernel::new();
        assert!(matches!(kernel.run(&worker, failing_next_to_fibonacci()), Err(Error::Jobs(e)) if e == ["failed"]));
        assert!(kernel.memo_count() < 11);
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[14]
    #[test]
    fn keep_going_collects_errors() {
        let deep_error = Mock::new(2, &[], |_| Response::Continuation(promise(1, vec![
            Mock::new(1, &[], |_| Response::Error("deep".into())),
        ])));
        let p = promise(3, vec![
            Mock::new(1, &[], |_| Response::Error("shallow".into())),
            deep_error,
            binop(3, 1, 2, |a, b| a + b),
        ]);
        let worker = SingleWorker::new();
        let mut kernel = Kernel::new().keep_going();
        let Err(Error::Jobs(mut errors)) = kernel.run(&worker, p) else { panic!("expected errors") };
        errors.sort();
        assert_eq!(errors, ["deep", "shallow"]);
    }
    // ~/~ end
    // ~/~ begin <<docs/src/engine.md#mock-tests>>[15]
    #[test]
    fn pool_cancel() {
        let barrier = Arc::new(Barrier::new(2));
        let cancelled = Arc::new(AtomicBool::new(false));
        let p = {
            let (b1, b2) = (barrier.clone(), barrier.clone());
            let cancelled = cancelled.clone();
            promise(3, vec![
                Mock::new(1, &[], move |_| {
                    b1.wait();
                    Response::Error("failed".into())
                }),
                Mock::cancellable(2, &[], move |_, token| {
                    b2.wait();
                    let start = Instant::now();
                    while start.elapsed() < Duration::from_secs(10) {
                        if token.is_cancelled() {
                            cancelled.store(true, Ordering::Relaxed);
                            break;
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                    Response::Value(0)
                }),
                binop(3, 1, 2, |a, b| a + b),
            ])
        };

        let start = Instant::now();
        assert!(matches!(eval_pool(p), Err(Error::Jobs(e)) if e == ["failed"]));
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(start.elapsed() < Duration::from_secs(5));
    }
    // ~/~ end
}
// ~/~ end
