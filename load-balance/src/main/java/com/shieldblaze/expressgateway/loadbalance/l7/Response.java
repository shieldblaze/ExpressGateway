package com.shieldblaze.expressgateway.loadbalance.l7;

import com.shieldblaze.expressgateway.backend.Backend;
import io.netty.handler.codec.http.HttpHeaders;

/**
 * {@link Response} contains selected {@link Backend} and {@link HttpHeaders} for {@link Request} response.
 */
public final class Response {
    private final Backend backend;
    private final HttpHeaders httpHeaders;

    /**
     * Create a {@link Response} Instance
     *
     * @param backend     {@link Backend}
     * @param httpHeaders {@link HttpHeaders}
     */
    public Response(Backend backend, HttpHeaders httpHeaders) {
        this.backend = backend;
        this.httpHeaders = httpHeaders;
    }

    /**
     * Get selected {@link Backend}
     */
    public Backend getBackend() {
        return backend;
    }

    /**
     * Get {@link HttpHeaders} for {@link Request} response.
     */
    public HttpHeaders getHTTPHeaders() {
        return httpHeaders;
    }
}
