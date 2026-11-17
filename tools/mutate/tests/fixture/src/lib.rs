pub mod other;

pub fn one() -> i32 { 1 }

pub fn five() -> i32 { 5 }

#[cfg(test)]
mod tests {
    #[test]
    fn one_is_one() {
        assert_eq!(super::one(), 1);
    }

    #[test]
    fn always_fails() {
        panic!("fails on healthy code, for the control-run test");
    }
}
