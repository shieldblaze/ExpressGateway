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
package com.shieldblaze.expressgateway.core.configuration.tls;

import com.shieldblaze.expressgateway.core.configuration.GenericConfiguration;
import io.netty.handler.ssl.SslContext;

import java.util.Map;

public final class TLSConfiguration extends GenericConfiguration {
    private Map<String, SslContext> hostnameCertificateMapping;
    private boolean enableOCSPCheck;
    private boolean enableOCSPStapling;
    private boolean forServer;

    public boolean enableOCSPCheck() {
        return enableOCSPCheck;
    }

    void enableOCSPCheck(boolean enableOCSPCheck) {
        this.enableOCSPCheck = enableOCSPCheck;
    }

    public boolean enableOCSPStapling() {
        return enableOCSPStapling;
    }

    void enableOCSPStapling(boolean enableOCSPStapling) {
        this.enableOCSPStapling = enableOCSPStapling;
    }

    public Map<String, SslContext> getHostnameCertificateMapping() {
        return hostnameCertificateMapping;
    }

    public SslContext getDefault() {
        return hostnameCertificateMapping.get("DEFAULT_HOST");
    }

    void setHostnameCertificateMapping(Map<String, SslContext> hostnameCertificateMapping) {
        this.hostnameCertificateMapping = hostnameCertificateMapping;
    }

    public boolean isForServer() {
        return forServer;
    }

    void setForServer(boolean forServer) {
        this.forServer = forServer;
    }
}
