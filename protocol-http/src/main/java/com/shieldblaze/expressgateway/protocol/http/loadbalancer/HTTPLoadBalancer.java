/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.protocol.http.loadbalancer;

import com.aayushatharva.brotli4j.encoder.Encoder;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.HttpServerInitializer;
import io.netty.handler.codec.compression.CompressionOptions;
import io.netty.handler.codec.compression.StandardCompressionOptions;

import java.net.InetSocketAddress;

/**
 * HTTP Load Balancer
 */
public class HTTPLoadBalancer extends L4LoadBalancer {

    private final CompressionOptions[] compressionOptions = new CompressionOptions[3];

    HTTPLoadBalancer(String name, InetSocketAddress bindAddress, L4FrontListener l4FrontListener,
                     ConfigurationContext configurationContext, HttpServerInitializer httpServerInitializer) {
        super(name, bindAddress, l4FrontListener, configurationContext, httpServerInitializer);
        httpServerInitializer.httpLoadBalancer(this);

        compressionOptions[0] = StandardCompressionOptions.brotli(new Encoder.Parameters().setQuality(httpConfiguration().brotliCompressionLevel()));
        compressionOptions[1] = StandardCompressionOptions.gzip(httpConfiguration().deflateCompressionLevel(), 15, 8);
        compressionOptions[2] = StandardCompressionOptions.deflate(httpConfiguration().deflateCompressionLevel(), 15, 8);
    }

    /**
     * Get {@link HttpConfiguration} Instance for this {@link HTTPLoadBalancer}
     */
    public HttpConfiguration httpConfiguration() {
        return configurationContext().httpConfiguration();
    }

    public CompressionOptions[] compressionOptions() {
        return compressionOptions;
    }

    @Override
    public String type() {
        return "L7/HTTP";
    }
}
