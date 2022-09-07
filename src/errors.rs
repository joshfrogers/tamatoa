use std::fmt;
use std::process::exit;

#[derive(Debug)]
pub enum ErrCode {
    ArgumentInvalid(&'static str),
    ZipError(u8)
}

#[allow(unreachable_patterns)]
impl fmt::Display for ErrCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            ErrCode::ArgumentInvalid(element) => write!(f, "ArgumentInvalid: {}", element),
            _ => write!(f, "{:?}", self),
        }
    }
}

impl ErrCode {
    pub fn get_retcode(&self) -> i32 {
        1
    }
}

pub fn exit_with_retcode(res: Result<(), ErrCode>) {
    match res {
        Ok(_) => {
            log::debug!("Exit without any error, returning 0");
            exit(0);
        }
        Err(e) => {
            let retcode = e.get_retcode();
            log::error!("Error on exit:\n\t{}\n\tReturning{}", e, retcode);
            exit(retcode);
        }
    }
}
