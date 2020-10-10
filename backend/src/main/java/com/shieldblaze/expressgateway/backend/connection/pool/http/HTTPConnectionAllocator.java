package com.shieldblaze.expressgateway.backend.connection.pool.http;

import stormpot.Allocator;
import stormpot.Slot;

public class HTTPConnectionAllocator implements Allocator<HTTPConnection> {

    private final int maxDataBacklog;

    public HTTPConnectionAllocator(int maxDataBacklog) {
        this.maxDataBacklog = maxDataBacklog;
    }

    @Override
    public HTTPConnection allocate(Slot slot) throws Exception {
        return new HTTPConnection(maxDataBacklog);
    }

    @Override
    public void deallocate(HTTPConnection httpConnection) throws Exception {
        httpConnection.disconnect();
    }
}
