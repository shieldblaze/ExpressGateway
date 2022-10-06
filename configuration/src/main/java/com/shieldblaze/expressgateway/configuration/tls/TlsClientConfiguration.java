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
package com.shieldblaze.expressgateway.configuration.tls;

import com.fasterxml.jackson.annotation.JsonIgnore;

/**
 * Configuration for TLS Client (ExpressGateway--to--Backend)
 */
public final class TlsClientConfiguration extends TlsConfiguration {

    @JsonIgnore
    private boolean validated;

    /**
     * This is the default implementation of {@link TlsClientConfiguration}
     * which is disabled by default.
     * </p>
     * <p>
     * To enable this, call {@link #enabled()}.
     */
    @JsonIgnore
    public static final TlsClientConfiguration DEFAULT = new TlsClientConfiguration();

    static {
        DEFAULT.ciphers = IntermediateCrypto.CIPHERS;
        DEFAULT.protocols = IntermediateCrypto.PROTOCOLS;
        DEFAULT.useStartTLS = false;
        DEFAULT.acceptAllCerts = false;
        DEFAULT.validated = true;
    }

    @Override
    public TlsConfiguration validate() throws IllegalArgumentException, NullPointerException {
        super.validate();
        validated = true;
        return this;
    }

    @Override
    public boolean validated() {
        return validated;
    }
}
