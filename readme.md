A Broadcaster (ssmr = single sender multi receiver) is a channel where one sender sends duplicate data to multiple receiver's.
the data is buffered (as specified by Broadcaster::subscribe(buffer_size: usize)) at the receiver and can be pulled with 'recv' or 'try_recv'.
All receivers will get sent a copy of the data, if a receivers buffer overflows it will block Broadcaster::send()
