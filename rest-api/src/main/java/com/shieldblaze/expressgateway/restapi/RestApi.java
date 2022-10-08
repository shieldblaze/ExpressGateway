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
package com.shieldblaze.expressgateway.restapi;

import com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoEntry;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.springframework.boot.Banner;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.boot.autoconfigure.mongo.MongoAutoConfiguration;
import org.springframework.boot.builder.SpringApplicationBuilder;
import org.springframework.context.ConfigurableApplicationContext;

import java.util.Objects;

@SpringBootApplication(exclude = MongoAutoConfiguration.class)
public class RestApi {
    private static final Logger logger = LogManager.getLogger(RestApi.class);
    private static ConfigurableApplicationContext ctx;
    protected static CryptoEntry CRYPTO_ENTRY;

    public static void start(CryptoEntry cryptoEntry) {
        CRYPTO_ENTRY = Objects.requireNonNull(cryptoEntry, "CryptoEntry cannot be 'null'");

        if (ctx == null) {
            ctx = new SpringApplicationBuilder()
                    .bannerMode(Banner.Mode.OFF)
                    .sources(RestApi.class)
                    .run();
        } else {
            ctx.stop();
            ctx = null;
            start(cryptoEntry);
        }
    }

    /**
     * Shutdown REST-API Server
     */
    public static void stop() {
        if (ctx != null) {
            ctx.stop();
            try {
                Thread.sleep(2500);
            } catch (InterruptedException e) {
                // This should never happen because we will never
                // interrupt this thread.
                logger.error(e);
            }
        }
    }
}
