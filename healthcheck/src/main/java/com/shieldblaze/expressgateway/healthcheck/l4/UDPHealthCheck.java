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
package com.shieldblaze.expressgateway.healthcheck.l4;

import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Arrays;

/**
 * <p> UDP based {@link HealthCheck} </p>
 * <p> How it works:
 * <ol>
 *     <li> It starts a UDP client and send "PING" packet to remote host. </li>
 *     <li> If remote host responds back with "PING" or "PONG" packet back,
 *              it'll pass the Health Check and close the UDP client. </li>
 *     <li> If remote host does not responds back with "PING" or "PONG" packet back,
 *              it'll fail the Health Check. </li>
 * </ol>
 * </p>
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

    @Override
    public void run() {
        try (DatagramSocket datagramSocket = new DatagramSocket()) {
            datagramSocket.setSoTimeout(timeout);
            DatagramPacket datagramPacket = new DatagramPacket(PING, 4, socketAddress);
            datagramSocket.send(datagramPacket);

            byte[] bytes = new byte[4];
            datagramPacket = new DatagramPacket(bytes, 4);
            datagramSocket.receive(datagramPacket);

            byte[] packet = Arrays.copyOfRange(datagramPacket.getData(), 0, 4);
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
