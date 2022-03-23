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
public final class TLSClientConfiguration extends TLSConfiguration {

    /**
     * This is the default implementation of {@link TLSClientConfiguration}
     * which is disabled by default.
     * </p>
     *
     * To enable this, call {@link #enabled()}.
     * The recommended way of enabling it to call {@link TLSConfigurationBuilder#copy(TLSConfiguration)}
     * or {@link TLSConfigurationBuilder#build()}
     *
     */
    @JsonIgnore
    public static final TLSClientConfiguration DEFAULT = new TLSClientConfiguration();

    static {
        DEFAULT.ciphers = IntermediateCrypto.CIPHERS;
        DEFAULT.protocols = IntermediateCrypto.PROTOCOLS;

        DEFAULT.useStartTLS = false;
        DEFAULT.acceptAllCerts = false;
    }

    @Override
    public String name() {
        return "TLSClientConfiguration";
    }
}
