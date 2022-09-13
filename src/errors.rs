use std::error::Error;
use std::fmt;
use std::process::exit;
use glob::PatternError;

#[derive(Debug)]
pub enum ErrCode {
    ArgumentInvalid(&'static str),
    UnsupportedPlatform(&'static str),
    PatternError(&'static str),
    IOError,
    ZipError(u8)
}

#[allow(unreachable_patterns)]
impl fmt::Display for ErrCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            ErrCode::ArgumentInvalid(element) => write!(f, "ArgumentInvalid: {}", element),
            _ => write!(f, "{:?}", self),
            ErrCode::UnsupportedPlatform(element) => write!(f, "Unsupported Platform: {}", element),
            _ => write!(f, "{:?}", self),
            ErrCode::PatternError(element) => write!(f, "Pattern Error: {}", element),
            _ => write!(f, "{:?}", self),
            ErrCode::ZipError(element) => write!(f, "Zip Errror: {}", element),
            _ => write!(f, "{:?}", self),
        }
    }
}

impl From<PatternError> for ErrCode {
    fn from(err: PatternError) -> ErrCode {
        ErrCode::PatternError(err.msg)
    }
}

impl From<std::io::Error> for ErrCode {
    fn from(err: std::io::Error) -> ErrCode {
        ErrCode::IOError
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
