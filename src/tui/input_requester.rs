use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use crate::plugin::ui::{
    ConfirmDialogData, DialogOption, InputRequest, PickerData, PickerItem as PluginPickerItem,
    PluginWidget, PluginWidgetResponse, TextInputData,
};
use crate::tools::traits::{InputError, InputRequester};

pub struct TuiInputRequester {
    request_tx: mpsc::Sender<InputRequest>,
}

impl TuiInputRequester {
    pub fn new(request_tx: mpsc::Sender<InputRequest>) -> Self {
        Self { request_tx }
    }
}

#[async_trait]
impl InputRequester for TuiInputRequester {
    async fn confirm(
        &self,
        title: &str,
        message: &str,
        options: &[(&str, &str)],
    ) -> Result<String, InputError> {
        let widget = PluginWidget::ConfirmDialog {
            data: ConfirmDialogData {
                title: title.into(),
                message: message.into(),
                options: options
                    .iter()
                    .map(|(id, label)| DialogOption {
                        id: id.to_string(),
                        label: label.to_string(),
                    })
                    .collect(),
                default: options.first().map(|(id, _)| id.to_string()),
            },
        };
        self.send_and_wait(widget).await
    }

    async fn pick(
        &self,
        title: &str,
        items: &[(String, String, String)],
    ) -> Result<String, InputError> {
        let widget = PluginWidget::Picker {
            data: PickerData {
                title: title.into(),
                items: items
                    .iter()
                    .map(|(id, label, desc)| PluginPickerItem {
                        id: id.clone(),
                        label: label.clone(),
                        description: desc.clone(),
                    })
                    .collect(),
            },
        };
        self.send_and_wait(widget).await
    }

    async fn text_input(
        &self,
        title: &str,
        prompt: &str,
        default: &str,
        masked: bool,
    ) -> Result<String, InputError> {
        let widget = PluginWidget::TextInput {
            data: TextInputData {
                title: title.into(),
                prompt: prompt.into(),
                default: default.into(),
                masked,
            },
        };
        self.send_and_wait(widget).await
    }
}

impl TuiInputRequester {
    async fn send_and_wait(&self, widget: PluginWidget) -> Result<String, InputError> {
        let (response_tx, response_rx) = oneshot::channel();
        let request = InputRequest {
            widget,
            response_tx,
        };
        self.request_tx
            .send(request)
            .await
            .map_err(|_| InputError::NotAvailable)?;

        match response_rx.await {
            Ok(PluginWidgetResponse::Selection { selection }) => Ok(selection),
            Ok(PluginWidgetResponse::SelectedItem { selected }) => Ok(selected.id),
            Ok(PluginWidgetResponse::TextValue { value }) => Ok(value),
            Ok(PluginWidgetResponse::Cancelled { .. }) => Err(InputError::Cancelled),
            Err(_) => Err(InputError::NotAvailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn send_confirm_and_receive_response() {
        let (tx, mut rx) = mpsc::channel::<InputRequest>(1);
        let requester = TuiInputRequester::new(tx);

        let handle = tokio::spawn(async move {
            let req = rx.recv().await.unwrap();
            assert!(matches!(req.widget, PluginWidget::ConfirmDialog { .. }));
            req.response_tx
                .send(PluginWidgetResponse::Selection {
                    selection: "yes".into(),
                })
                .unwrap();
        });

        let result = requester
            .confirm("Delete?", "Are you sure?", &[("yes", "Yes"), ("no", "No")])
            .await
            .unwrap();
        assert_eq!(result, "yes");
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_returns_error() {
        let (tx, mut rx) = mpsc::channel::<InputRequest>(1);
        let requester = TuiInputRequester::new(tx);

        let handle = tokio::spawn(async move {
            let req = rx.recv().await.unwrap();
            req.response_tx
                .send(PluginWidgetResponse::Cancelled { cancelled: true })
                .unwrap();
        });

        let result = requester.confirm("Q", "Q?", &[("a", "A")]).await;
        assert!(matches!(result, Err(InputError::Cancelled)));
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn dropped_sender_returns_not_available() {
        let (tx, rx) = mpsc::channel::<InputRequest>(1);
        let requester = TuiInputRequester::new(tx);
        drop(rx);

        let result = requester.confirm("Q", "Q?", &[("a", "A")]).await;
        assert!(matches!(result, Err(InputError::NotAvailable)));
    }
}
