//! wasi-sql implementation for Azure Table storage

use std::sync::Arc;

use azure_data_tables::prelude::TableClient;
use futures::future::FutureExt;
use qwasr_wasi_sql::{Connection, DataType, FutureResult, Row, WasiSqlCtx};

use crate::Client;

impl WasiSqlCtx for Client {
    fn open(&self, name: String) -> FutureResult<Arc<dyn Connection>> {
        tracing::debug!("opening connection to azure storage table {name}");

        let table_client = self.client.table_client(name);
        async move {
            Ok(Arc::new(AzTableConnection(Arc::new(table_client))) as Arc<dyn Connection>)
        }.boxed()
    }
}

#[derive(Debug)]
pub struct AzTableConnection(Arc<TableClient>);

impl Connection for AzTableConnection {
    fn query(&self, query: String, params: Vec<DataType>) -> FutureResult<Vec<Row>> {
        tracing::debug!("query: {query}, params: {params:?}");
        let cnn = Arc::clone(&self.0);
        todo!()
    }

    fn exec(&self, query: String, params: Vec<DataType>) -> FutureResult<u32> {
        tracing::debug!("exec: {query}, params: {params:?}");
        todo!()
    }
}