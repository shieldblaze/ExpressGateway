/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.restapi;

import io.netty.util.internal.SystemPropertyUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.springframework.boot.web.embedded.undertow.ConfigurableUndertowWebServerFactory;
import org.springframework.boot.web.server.Compression;
import org.springframework.boot.web.server.Http2;
import org.springframework.boot.web.server.WebServerFactoryCustomizer;
import org.springframework.stereotype.Component;
import org.springframework.util.unit.DataSize;

import java.net.InetAddress;
import java.net.UnknownHostException;

@Component
final class WebServerCustomizer implements WebServerFactoryCustomizer<ConfigurableUndertowWebServerFactory> {

    private static final Logger logger = LogManager.getLogger(WebServerCustomizer.class);

    @Override
    public void customize(ConfigurableUndertowWebServerFactory container) {
        try {
            container.setAddress(InetAddress.getByName(SystemPropertyUtil.get("restapi.bindAddress", "127.0.0.1")));
        } catch (UnknownHostException e) {
            logger.error("Caught Error at WebServerCustomizer", e);
            System.exit(1);
        }

        container.setPort(SystemPropertyUtil.getInt("restapi.bindPort", 9110));
        container.setIoThreads(Runtime.getRuntime().availableProcessors());
        container.setWorkerThreads(Runtime.getRuntime().availableProcessors() * 2);
        container.setUseDirectBuffers(true);

        Compression compression = new Compression();
        compression.setEnabled(true);
        compression.setMinResponseSize(DataSize.ofBytes(500));
        container.setCompression(compression);

        Http2 http2 = new Http2();
        http2.setEnabled(true);
        container.setHttp2(http2);
    }
}
