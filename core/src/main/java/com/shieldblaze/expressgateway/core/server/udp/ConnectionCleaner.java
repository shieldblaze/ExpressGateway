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
package com.shieldblaze.expressgateway.core.server.udp;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.Map;

/**
 * Removes inactive {@link Connection} from {@link UpstreamHandler#connectionMap}
 */
final class ConnectionCleaner extends Thread {

    private static final Logger logger = LogManager.getLogger(ConnectionCleaner.class);

    private boolean runService = false;
    private final UpstreamHandler upstreamHandler;

    public ConnectionCleaner(UpstreamHandler upstreamHandler) {
        super("Connection-Cleaner-Service");
        this.upstreamHandler = upstreamHandler;
    }

    @Override
    public void run() {

        while (runService) {

            for (Map.Entry<InetSocketAddress, Connection> entry : upstreamHandler.connectionMap.entrySet()) {
                if (!entry.getValue().connectionActive.get()) {
                    entry.getValue().clearBacklog();
                    upstreamHandler.connectionMap.remove(entry.getKey());
                }
            }

            try {
                sleep(1000L);
            } catch (InterruptedException e) {
                logger.error("Caught Error At ConnectionCleaner", e);
            }
        }
    }

    public void startService() {
        if (!runService) {
            start();
            runService = true;
        }
    }

    public void stopService() {
        runService = false;
    }
}
