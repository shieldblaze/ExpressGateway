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
package com.shieldblaze.expressgateway.healthcheck.l4;

import com.shieldblaze.expressgateway.healthcheck.HealthCheck;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Arrays;

/**
 * UDP based {@link HealthCheck}.
 *
 * <p>Sends a PING packet and expects a PING or PONG response within timeout.</p>
 */
public final class UDPHealthCheck extends HealthCheck {

    private static final byte[] PING = "PING".getBytes();
    private static final byte[] PONG = "PONG".getBytes();

    public UDPHealthCheck(InetSocketAddress socketAddress, Duration timeout) {
        super(socketAddress, timeout);
    }

    public UDPHealthCheck(InetSocketAddress socketAddress, Duration timeout, int samples) {
        super(socketAddress, timeout, samples);
    }

    public UDPHealthCheck(InetSocketAddress socketAddress, Duration timeout, int samples,
                          int rise, int fall) {
        super(socketAddress, timeout, samples, rise, fall,
                Duration.ofSeconds(5), 1000L, 60_000L);
    }

    @Override
    public void run() {
        try (DatagramSocket datagramSocket = new DatagramSocket()) {
            datagramSocket.setSoTimeout(timeout);
            DatagramPacket datagramPacket = new DatagramPacket(PING, 4, socketAddress);
            datagramSocket.send(datagramPacket);

            byte[] bytes = new byte[64];
            datagramPacket = new DatagramPacket(bytes, bytes.length);
            datagramSocket.receive(datagramPacket);

            byte[] packet = Arrays.copyOfRange(datagramPacket.getData(), 0, datagramPacket.getLength());
            if (Arrays.equals(packet, PING) || Arrays.equals(packet, PONG)) {
                markSuccess();
            } else {
                markFailure();
            }
        } catch (Exception e) {
            markFailure();
        }
    }
}
