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

import java.net.InetSocketAddress;
import java.net.Socket;
import java.time.Duration;

/**
 * TCP based {@link HealthCheck}.
 *
 * <p>Connects to the remote host via TCP. If the connection succeeds
 * within the timeout, the check passes. Both connect timeout and
 * SO_TIMEOUT are enforced.</p>
 */
public final class TCPHealthCheck extends HealthCheck {

    public TCPHealthCheck(InetSocketAddress socketAddress, Duration timeout) {
        super(socketAddress, timeout);
    }

    public TCPHealthCheck(InetSocketAddress socketAddress, Duration timeout, int samples) {
        super(socketAddress, timeout, samples);
    }

    public TCPHealthCheck(InetSocketAddress socketAddress, Duration timeout, int samples,
                          int rise, int fall) {
        super(socketAddress, timeout, samples, rise, fall,
                Duration.ofSeconds(5), 1000L, 60_000L);
    }

    @Override
    public void run() {
        try (Socket socket = new Socket()) {
            socket.setSoTimeout(timeout);
            socket.setReuseAddress(true);
            socket.connect(socketAddress, timeout);
            if (socket.isConnected()) {
                markSuccess();
            } else {
                markFailure();
            }
        } catch (Exception e) {
            markFailure();
        }
    }
}
