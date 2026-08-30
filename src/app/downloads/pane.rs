use relm4::prelude::*;

use crate::models::download::{DownloadFilter, DownloadSort};
use crate::ui::downloads_pane::{
    DownloadsPane, DownloadsPaneInit, DownloadsPaneOutput, DownloadsPaneState,
};

use super::super::App;
use super::super::messages::{AppMsg, DownloadsMsg};

pub(crate) fn launch(
    scroll: gtk::ScrolledWindow,
    list: gtk::ListBox,
    sender: &ComponentSender<App>,
) -> Controller<DownloadsPane> {
    DownloadsPane::builder()
        .launch(DownloadsPaneInit {
            state: DownloadsPaneState {
                filter: DownloadFilter::All,
                sort: DownloadSort::Default,
                show_hidden: false,
                active_count: 0,
                completed_count: 0,
                is_empty: true,
            },
            scroll,
            list,
        })
        .forward(sender.input_sender(), |output| match output {
            DownloadsPaneOutput::SetFilter(filter) => {
                AppMsg::Downloads(DownloadsMsg::SetDownloadFilter(filter))
            }
            DownloadsPaneOutput::SetSort(sort) => {
                AppMsg::Downloads(DownloadsMsg::DownloadSortChanged(sort))
            }
            DownloadsPaneOutput::Scan => AppMsg::Downloads(DownloadsMsg::ScanDownloadsFolder),
            DownloadsPaneOutput::SetShowHidden(show) => {
                AppMsg::Downloads(DownloadsMsg::SetShowHiddenDownloads(show))
            }
        })
}
