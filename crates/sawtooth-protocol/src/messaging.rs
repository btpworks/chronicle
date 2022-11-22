use std::{collections::HashSet, sync::Arc};

use crate::{
    address::{SawtoothAddress, FAMILY, VERSION},
    messages::MessageBuilder,
    sawtooth::{ClientBatchSubmitRequest, ClientBatchSubmitResponse, Transaction},
};

use common::{
    k256::ecdsa::SigningKey,
    ledger::{LedgerWriter, SubmissionError},
    protocol::ProtocolError,
    prov::{operations::ChronicleOperation, ChronicleTransactionId, ProcessorError},
};
use custom_error::*;
use derivative::Derivative;
use prost::Message as ProstMessage;

use sawtooth_sdk::{
    messages::validator::Message_MessageType,
    messaging::{
        stream::{MessageConnection, MessageReceiver, MessageSender, ReceiveError, SendError},
        zmq_stream::{ZmqMessageConnection, ZmqMessageSender},
    },
};
use tokio::task::JoinError;
use tracing::{debug, instrument, trace};

#[derive(Derivative)]
#[derivative(Debug)]
pub struct SawtoothSubmitter {
    #[derivative(Debug = "ignore")]
    tx: ZmqMessageSender,
    rx: MessageReceiver,
    builder: MessageBuilder,
}

custom_error! {pub SawtoothSubmissionError
    Send{source: SendError}                                 = "Submission failed to send to validator",
    Recv{source: ReceiveError}                              = "Submission failed to send to validator",
    UnexpectedStatus{status: i32}                           = "Validator status unexpected {status}",
    Join{source: JoinError}                                 = "Submission blocking thread pool",
    Ld{source: ProcessorError}                              = "Json LD processing",
    Protocol{source: ProtocolError}                         = "Protocol {source}",
}

/// The sawtooth futures and their sockets are not controlled by a compatible reactor
impl SawtoothSubmitter {
    #[allow(dead_code)]
    pub fn new(address: &url::Url, signer: &SigningKey) -> Self {
        let builder = MessageBuilder::new(signer.to_owned(), FAMILY, VERSION);
        let (tx, rx) = ZmqMessageConnection::new(address.as_str()).create();
        SawtoothSubmitter { tx, rx, builder }
    }

    async fn make_tx(
        &mut self,
        transactions: &[ChronicleOperation],
    ) -> Result<(Transaction, ChronicleTransactionId), ProtocolError> {
        let addresses = transactions
            .iter()
            .flat_map(|tx| tx.dependencies())
            .map(|addr| (SawtoothAddress::from(&addr).to_string(), addr))
            .collect::<HashSet<_>>();

        debug!(address_map = ?addresses);

        self.builder
            .make_sawtooth_transaction(
                addresses.iter().map(|x| x.0.clone()).collect(),
                addresses.into_iter().map(|x| x.0).collect(),
                vec![],
                transactions,
            )
            .await
    }

    #[instrument(
        name = "submit_sawtooth_tx",
        level = "info",
        skip(self, transactions),
        ret(Debug)
    )]
    async fn submit(
        &mut self,
        transactions: &[ChronicleOperation],
    ) -> Result<ChronicleTransactionId, (ChronicleTransactionId, SawtoothSubmissionError)> {
        // Practically, a protobuf serialization error here is probably a crash
        // loop level fault, but we will handle it without panic for now
        let (sawtooth_transaction, tx_id) = self
            .make_tx(transactions)
            .await
            .map_err(|e| (ChronicleTransactionId::from(""), e.into()))?;

        let ret_tx_id = tx_id.clone();

        let res = async move {
            let batch = self.builder.wrap_tx_as_sawtooth_batch(sawtooth_transaction);

            trace!(?batch, "Validator request");

            let request = ClientBatchSubmitRequest {
                batches: vec![batch],
            };

            let mut future = self.tx.send(
                Message_MessageType::CLIENT_BATCH_SUBMIT_REQUEST,
                &tx_id.to_string(),
                &request.encode_to_vec(),
            )?;

            debug!(submit_transaction=%tx_id);

            let result = future.get_timeout(std::time::Duration::from_secs(10))?;

            let response =
                ClientBatchSubmitResponse::decode(&*result.content).map_err(ProtocolError::from)?;

            debug!(validator_response=?response);

            if response.status == 1 {
                Ok(tx_id)
            } else {
                Err(SawtoothSubmissionError::UnexpectedStatus {
                    status: response.status,
                })
            }
        }
        .await;

        res.map_err(|e| (ret_tx_id, e))
    }
}

#[async_trait::async_trait(?Send)]
impl LedgerWriter for SawtoothSubmitter {
    /// TODO: This blocks on a bunch of non tokio / futures 'futures' in the sawtooth rust SDK,
    /// which also exposes a bunch of non clonable types so we probably need another dispatch / join mpsc here
    async fn submit(
        &mut self,
        tx: &[ChronicleOperation],
    ) -> Result<ChronicleTransactionId, SubmissionError> {
        self.submit(tx)
            .await
            .map_err(|(tx_id, e)| SubmissionError::implementation(&tx_id, Arc::new(e.into())))
    }
}