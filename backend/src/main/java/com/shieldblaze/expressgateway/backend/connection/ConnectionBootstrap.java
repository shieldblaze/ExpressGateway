package com.shieldblaze.expressgateway.backend.connection;

import io.netty.bootstrap.Bootstrap;

/**
 * {@link ConnectionBootstrap} used methods for creating {@link Bootstrap}
 */
public interface ConnectionBootstrap {
    Bootstrap bootstrap();
}
