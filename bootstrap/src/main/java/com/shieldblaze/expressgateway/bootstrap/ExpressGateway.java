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
package com.shieldblaze.expressgateway.bootstrap;

import com.amazon.corretto.crypto.provider.AmazonCorrettoCryptoProvider;
import com.amazon.corretto.crypto.provider.SelfTestStatus;
import com.shieldblaze.expressgateway.restapi.RestAPI;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.UUID;

/**
 * This class initializes and boots up the ExpressGateway.
 */
public final class ExpressGateway {

    private static final Logger logger = LogManager.getLogger(ExpressGateway.class);

    static {
        if (System.getProperty("log4j.configurationFile") == null) {
            System.setProperty("log4j.configurationFile", "log4j2.xml");
        }

        AmazonCorrettoCryptoProvider.install();

        Throwable throwable = AmazonCorrettoCryptoProvider.INSTANCE.getLoadingError();
        if (throwable == null && AmazonCorrettoCryptoProvider.INSTANCE.runSelfTests().equals(SelfTestStatus.PASSED)) {
            logger.info("AmazonCorrettoCryptoProvider is successfully installed");
        } else {
            logger.error("Failed to install AmazonCorrettoCryptoProvider", throwable);
        }
    }

    /**
     * Timestamp of ExpressGateway startup
     */
    public static final long STARTED_AT = System.currentTimeMillis();

    /**
     * UUID of this ExpressGateway runtime-instance
     */
    public static final String ID = UUID.randomUUID().toString();

    public static void main(String[] args) {
        RestAPI.start();
    }
}
