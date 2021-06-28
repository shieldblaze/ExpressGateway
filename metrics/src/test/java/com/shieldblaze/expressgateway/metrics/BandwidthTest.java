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

import io.netty.util.internal.PlatformDependent;
import org.junit.Assume;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.UnknownHostException;

import static org.junit.jupiter.api.Assertions.*;
import static org.junit.jupiter.api.Assumptions.assumeFalse;

class BandwidthTest {

    @BeforeAll
    static void setup() throws IOException, InterruptedException {
        assumeFalse(PlatformDependent.isWindows());
        assumeFalse(PlatformDependent.isOsx());

        ServerSocket serverSocket = new ServerSocket(0, 1000, InetAddress.getByName("127.0.0.1"));
        Socket socket = new Socket("127.0.0.1", serverSocket.getLocalPort());
        socket.getOutputStream().write(0);
        socket.getOutputStream().write(1);
        socket.getOutputStream().write(2);
        socket.getOutputStream().write(3);
        socket.getOutputStream().write(4);
        socket.getOutputStream().write(5);
        socket.close();
        serverSocket.close();
    }

    @Test
    void fetchBytesReadWriteTest() throws InterruptedException {
        BandwidthMetric bandwidthMetric = new Bandwidth("lo");
        Thread.sleep(2000L);
        System.err.println(bandwidthMetric.rx());
        System.err.println(bandwidthMetric.tx());
        assertTrue(bandwidthMetric.rx() > 0);
        assertTrue(bandwidthMetric.tx() > 0);
    }
}
