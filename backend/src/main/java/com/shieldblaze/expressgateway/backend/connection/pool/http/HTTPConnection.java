package com.shieldblaze.expressgateway.backend.connection.pool.http;

import com.shieldblaze.expressgateway.backend.connection.Connection;
import io.netty.channel.ChannelFuture;
import io.netty.handler.codec.http.HttpObject;

public class HTTPConnection extends Connection<HttpObject> {

    public HTTPConnection(int maxDataBacklog) {
        super(maxDataBacklog);
    }

    @Override
    protected ChannelFuture writeData(HttpObject data) {
        return super.writeData(data);
    }
}
