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

import java.net.InetSocketAddress;
import java.net.Socket;

public final class TCP extends HealthCheck {

    public TCP(InetSocketAddress socketAddress, int timeout) {
        super(socketAddress, timeout);
    }

    public TCP(InetSocketAddress socketAddress, int timeout, int samples) {
        super(socketAddress, timeout, samples);
    }

    @Override
    public void check() {
        try (Socket socket = new Socket()) {
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
