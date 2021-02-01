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
package com.shieldblaze.expressgateway.backend;

import com.shieldblaze.expressgateway.common.utils.Number;

import java.util.Objects;

public final class HealthCheckTemplate {

    /**
     * Health Check Protocol
     */
    private Protocol protocol;

    /**
     * Health Check Port
     */
    private int port;

    /**
     * HTTP(s) Path
     */
    private String path;

    /**
     * Timeout in seconds
     */
    private int timeout;

    /**
     * Number of Health Check samples
     */
    private int samples;

    public HealthCheckTemplate(Protocol protocol, int port, String path, int timeout, int samples) {
        protocol(protocol);
        path(path);
        timeout(timeout);
        samples(samples);
        port(port);
    }

    public Protocol protocol() {
        return protocol;
    }

    public void protocol(Protocol protocol) {
        this.protocol = Objects.requireNonNull(protocol, "Protocol");
    }

    public int port() {
        return port;
    }

    public void port(int port) {
        this.port = Number.checkRange(port, 1, 65535, "Port");
    }

    public String path() {
        return path;
    }

    public void path(String path) {
        this.path = Objects.requireNonNull(path, "Path");
    }

    public int timeout() {
        return timeout;
    }

    public void timeout(int timeout) {
        this.timeout = Number.checkPositive(timeout, "Timeout");
    }

    public int samples() {
        return samples;
    }

    public void samples(int samples) {
        this.samples = Number.checkPositive(samples, "Samples");
    }

    public enum Protocol {
        TCP,
        UDP,
        HTTP,
        HTTPS
    }
}
