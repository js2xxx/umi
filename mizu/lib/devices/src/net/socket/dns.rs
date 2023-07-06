use smoltcp::socket::dns::{GetQueryResultError, StartQueryError};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Error {
    Start(StartQueryError),
    Query(GetQueryResultError),
}

impl From<StartQueryError> for Error {
    fn from(value: StartQueryError) -> Self {
        Error::Start(value)
    }
}

impl From<GetQueryResultError> for Error {
    fn from(value: GetQueryResultError) -> Self {
        Error::Query(value)
    }
}