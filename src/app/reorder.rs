pub(super) fn item_destination(from: usize, insertion: usize, len: usize) -> Option<usize> {
    if from >= len || insertion > len {
        return None;
    }

    Some(if insertion > from {
        insertion - 1
    } else {
        insertion
    })
}

pub(super) fn block_destination(selected: &[usize], insertion: usize, len: usize) -> Option<usize> {
    if selected.is_empty()
        || insertion > len
        || selected.windows(2).any(|pair| pair[0] >= pair[1])
        || selected.iter().any(|&index| index >= len)
    {
        return None;
    }

    let removed_before = selected.partition_point(|&index| index < insertion);
    Some((insertion - removed_before).min(len - selected.len()))
}

#[cfg(test)]
mod tests {
    use super::{block_destination, item_destination};

    #[test]
    fn moving_item_down_accounts_for_removed_row() {
        assert_eq!(item_destination(1, 4, 5), Some(3));
    }

    #[test]
    fn moving_item_up_keeps_insertion_index() {
        assert_eq!(item_destination(4, 1, 5), Some(1));
    }

    #[test]
    fn moving_item_to_end_uses_last_index() {
        assert_eq!(item_destination(1, 5, 5), Some(4));
    }

    #[test]
    fn moving_block_down_accounts_for_every_removed_row() {
        assert_eq!(block_destination(&[1, 2], 5, 6), Some(3));
    }

    #[test]
    fn moving_block_up_keeps_relative_order() {
        assert_eq!(block_destination(&[3, 4], 1, 6), Some(1));
    }

    #[test]
    fn dropping_inside_selected_block_is_unchanged() {
        assert_eq!(block_destination(&[1, 2], 2, 5), Some(1));
        assert_eq!(block_destination(&[1, 2], 3, 5), Some(1));
    }
}
