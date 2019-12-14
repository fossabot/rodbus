use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::channel::{Request, ServiceRequest};
use crate::error::details::{InvalidRequest, ResponseParseError};
use crate::error::*;
use crate::service::services::*;
use crate::service::traits::Service;

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct UnitId {
    id: u8,
}

pub struct AddressRange {
    pub start: u16,
    pub count: u16
}

mod constants {
    pub const ON : u16 = 0xFF00;
    pub const OFF : u16 = 0x0000;
}

#[repr(u16)]
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum CoilState {
    On = constants::ON,
    Off = constants::OFF
}

impl CoilState {
    pub fn from_bool(value : bool) -> Self {
        if value { CoilState::On } else { CoilState::Off }
    }

    pub fn from_u16(value : u16) -> Result<Self, ResponseParseError> {
        match value {
            constants::ON => Ok(CoilState::On),
            constants::OFF => Ok(CoilState::Off),
            _ => Err(ResponseParseError::UnknownCoilState(value))
        }
    }

    pub fn to_u16(&self) -> u16 {
        *self as u16
    }
}

#[derive(PartialEq)]
pub struct RegisterValue {
    pub value : u16
}

impl RegisterValue {
    pub fn new(value : u16) -> Self {
        RegisterValue { value }
    }
}

impl AddressRange {

    pub const MAX_REGISTERS : u16 = 125;
    pub const MAX_BINARY_BITS : u16 = 2000;

    pub fn new(start: u16, count: u16) -> Self {
        AddressRange { start, count }
    }

    fn check_validity(&self, max_count: u16) -> Result<(), InvalidRequest> {
        // a count of zero is never valid
        if self.count == 0 {
            return Err(InvalidRequest::CountOfZero);
        }

        // check that start/count don't overflow u16
        let last_address = (self.start as u32) + (self.count as u32 - 1);
        if last_address > (std::u16::MAX as u32) {
            return Err(InvalidRequest::AddressOverflow(self.start, self.count));
        }

        if self.count > max_count {
            return Err(InvalidRequest::CountTooBigForType(self.count, max_count));
        }

        Ok(())
    }

    pub fn check_validity_for_bits(&self) -> Result<(), InvalidRequest> {
        self.check_validity(Self::MAX_BINARY_BITS)
    }

    pub fn check_validity_for_registers(&self) -> Result<(), InvalidRequest> {
        self.check_validity(Self::MAX_REGISTERS)
    }
}

#[derive(PartialEq)]
pub struct Indexed<T> {
    pub index: u16,
    pub value: T
}

impl<T> Indexed<T> {
    pub fn new(index: u16, value : T) -> Self {
        Indexed {  index, value }
    }
}

impl UnitId {
    pub fn new(unit_id: u8) -> Self {
        Self { id: unit_id }
    }

    pub fn default() -> Self {
        Self { id: 0xFF }
    }

    pub fn value(&self) -> u8 {
        self.id
    }
}

#[derive(Clone)]
pub struct Session {
    id: UnitId,
    response_timeout: Duration,
    request_channel: mpsc::Sender<Request>,
}

#[derive(Clone)]
pub struct CallbackSession {
    inner: Session
}

impl Session {
    pub(crate) fn new(id: UnitId, response_timeout: Duration, request_channel: mpsc::Sender<Request>) -> Self {
        Session { id, response_timeout, request_channel }
    }

    async fn make_service_call<S : Service>(&mut self, request: S::Request) -> Result<S::Response, Error> {
        S::check_request_validity(&request)?;
        let (tx, rx) = oneshot::channel::<Result<S::Response, Error>>();
        let request = S::create_request(ServiceRequest::new(self.id, self.response_timeout,request, tx));
        self.request_channel.send(request).await.map_err(|_| ErrorKind::Shutdown)?;
        rx.await.map_err(|_| ErrorKind::Shutdown)?
    }

    pub async fn read_coils(&mut self, range: AddressRange) -> Result<Vec<Indexed<bool>>, Error> {
        self.make_service_call::<ReadCoils>(range).await
    }

    pub async fn read_discrete_inputs(&mut self, range: AddressRange) -> Result<Vec<Indexed<bool>>, Error> {
        self.make_service_call::<ReadDiscreteInputs>(range).await
    }

    pub async fn read_holding_registers(&mut self, range: AddressRange) -> Result<Vec<Indexed<u16>>, Error> {
        self.make_service_call::<ReadHoldingRegisters>(range).await
    }

    pub async fn read_input_registers(&mut self, range: AddressRange) -> Result<Vec<Indexed<u16>>, Error> {
        self.make_service_call::<ReadInputRegisters>(range).await
    }

    pub async fn write_single_coil(&mut self, value: Indexed<CoilState>) -> Result<Indexed<CoilState>, Error> {
        self.make_service_call::<WriteSingleCoil>(value).await
    }

    pub async fn write_single_register(&mut self, value: Indexed<RegisterValue>) -> Result<Indexed<RegisterValue>, Error> {
        self.make_service_call::<WriteSingleRegister>(value).await
    }
}

pub trait Handler<T> {
    fn handle(&self, result: T);
}

impl CallbackSession {

    pub fn new(inner : Session) -> Self {
        CallbackSession { inner }
    }

    fn start_request<S: Service + 'static>(&mut self, request: S::Request, callback: Box<dyn Handler<Result<S::Response, Error>> + Send + Sync>) -> ()  {
        let mut session = self.inner.clone();
        tokio::spawn(
            async move {
                callback.handle(session.make_service_call::<S>(request).await);
            }
        );
    }

    pub fn read_coils(&mut self, range: AddressRange, callback: Box<dyn Handler<Result<Vec<Indexed<bool>>, Error>> + Send + Sync>) -> () {
        self.start_request::<ReadCoils>(range, callback);
    }
}