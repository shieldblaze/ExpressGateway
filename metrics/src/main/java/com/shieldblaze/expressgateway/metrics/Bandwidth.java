/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.metrics;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.nio.file.Files;
import java.nio.file.Path;

/**
 * System Network Bandwidth Metric
 */
public class Bandwidth extends Thread implements Closeable {

    private static final Logger logger = LogManager.getLogger(Bandwidth.class);

    private long RX;
    private long TX;
    private boolean stop;
    private final Path rx;
    private final Path tx;

    public Bandwidth(String ifName) {
        super("IF-" + ifName + "; Packets-Monitor-Thread");
        rx = Path.of("/sys/class/net/" + ifName + "/statistics/rx_bytes");
        tx = Path.of("/sys/class/net/" + ifName + "/statistics/tx_bytes");
    }

    @SuppressWarnings("BusyWait")
    @Override
    public void run() {
        while (!stop) {
            try {
                long rx_old = Long.parseLong(Files.readString(rx).trim());
                long tx_old = Long.parseLong(Files.readString(tx).trim());

                sleep(1000); // Wait for 1 second

                long rx_new = Long.parseLong(Files.readString(rx).trim());
                long tx_new = Long.parseLong(Files.readString(tx).trim());

                RX  = rx_new - rx_old;
                TX =  tx_new - tx_old;
            } catch (Exception ex) {
                logger.error(ex);
            }
        }
    }

    public long rx() {
        return RX;
    }

    public long tx() {
        return TX;
    }

    @Override
    public void close() {
        stop = true;
    }
}
