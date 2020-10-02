package com.shieldblaze.expressgateway.core.server.http;

import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;

final class HeaderUtils {

    static void setGenericHeaders(HttpHeaders headers) {
        headers.set(HttpHeaderNames.SERVER, "ShieldBlaze ExpressGateway");
    }
}
