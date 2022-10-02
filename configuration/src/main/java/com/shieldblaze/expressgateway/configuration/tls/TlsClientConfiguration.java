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
import com.fasterxml.jackson.annotation.JsonProperty;
import dev.morphia.annotations.Entity;
import dev.morphia.annotations.Id;
import dev.morphia.annotations.Transient;

import java.util.UUID;

/**
 * Configuration for TLS Client (ExpressGateway--to--Backend)
 */
@Entity(value = "TlsClient", useDiscriminator = false)
public final class TlsClientConfiguration extends TlsConfiguration {

    @Id
    @JsonProperty
    String id;

    @Transient
    @JsonIgnore
    private boolean validated;

    /**
     * This is the default implementation of {@link TlsClientConfiguration}
     * which is disabled by default.
     * </p>
     *
     * To enable this, call {@link #enabled()}.
     */
    @JsonIgnore
    public static final TlsClientConfiguration DEFAULT = new TlsClientConfiguration();

    static {
        DEFAULT.id = "default";
        DEFAULT.ciphers = IntermediateCrypto.CIPHERS;
        DEFAULT.protocols = IntermediateCrypto.PROTOCOLS;
        DEFAULT.useStartTLS = false;
        DEFAULT.acceptAllCerts = false;
        DEFAULT.validated = true;
    }

    @Override
    public TlsConfiguration validate() throws IllegalArgumentException, NullPointerException {
        if (id == null) {
            id = UUID.randomUUID().toString();
        }
        super.validate();
        validated = true;
        return this;
    }

    @Override
    public String id() {
        return id;
    }

    @Override
    public boolean validated() {
        return validated;
    }
}
