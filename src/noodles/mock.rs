// ~/~ begin <<docs/src/engine.md#src/noodles/mock.rs>>[init]
use std::collections::HashMap;
use std::sync::Arc;
use super::task::{Task, Promise, Response, Error, InternalError};
use super::kernel::Kernel;
use super::single_worker::SingleWorker;

type Func = Arc<dyn Fn(&[i64]) -> Response<Mock> + Send + Sync>;

struct Mock {
    key: u64,
    deps: Vec<u64>,
    args: HashMap<u64, i64>,
    func: Func,
}

impl Mock {
    fn new(key: u64, deps: &[u64], func: impl Fn(&[i64]) -> Response<Mock> + Send + Sync + 'static) -> Self {
        Mock { key, deps: deps.to_vec(), args: HashMap::new(), func: Arc::new(func) }
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

    fn run(self) -> Response<Self> {
        let args: Vec<i64> = self.deps.iter().map(|k| self.args[k]).collect();
        (self.func)(&args)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(p: Box<Promise<Mock>>) -> Result<i64, Error<Mock>> {
        let worker = SingleWorker::new();
        let mut kernel = Kernel::new();
        let result = kernel.run(&worker, p);
        assert_eq!(kernel.frame_count(), 0);
        result
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
        assert!(matches!(eval(p), Err(Error::Job(e)) if e == "division by zero"));
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
}
// ~/~ end
