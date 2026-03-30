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
package com.shieldblaze.expressgateway.controlplane.config.types;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;

import java.util.Objects;

/**
 * Configuration spec for TLS certificate material.
 *
 * <p>Stores PEM-encoded certificate, private key, and optional chain.
 * Used by listeners that terminate TLS (required for HTTP/3 per RFC 9114,
 * optional for HTTP/1.1 and HTTP/2).</p>
 *
 * @param name        The certificate name
 * @param certificate PEM-encoded certificate
 * @param privateKey  PEM-encoded private key
 * @param chain       PEM-encoded certificate chain (optional, null if not provided)
 * @param autoRenew   Whether to automatically renew this certificate (e.g. via ACME)
 */
public record TlsCertSpec(
        @JsonProperty("name") String name,
        @JsonProperty("certificate") String certificate,
        @JsonProperty("privateKey") String privateKey,
        @JsonProperty("chain") String chain,
        @JsonProperty("autoRenew") boolean autoRenew
) implements ConfigSpec {

    private static final String PEM_BEGIN = "-----BEGIN ";

    @Override
    public void validate() {
        Objects.requireNonNull(name, "name");
        if (name.isBlank()) {
            throw new IllegalArgumentException("name must not be blank");
        }
        Objects.requireNonNull(certificate, "certificate");
        if (certificate.isBlank()) {
            throw new IllegalArgumentException("certificate must not be blank");
        }
        if (!certificate.contains(PEM_BEGIN)) {
            throw new IllegalArgumentException("certificate does not appear to be PEM-encoded");
        }
        Objects.requireNonNull(privateKey, "privateKey");
        if (privateKey.isBlank()) {
            throw new IllegalArgumentException("privateKey must not be blank");
        }
        if (!privateKey.contains(PEM_BEGIN)) {
            throw new IllegalArgumentException("privateKey does not appear to be PEM-encoded");
        }
        // chain is optional but if present must be PEM
        if (chain != null && !chain.isBlank() && !chain.contains(PEM_BEGIN)) {
            throw new IllegalArgumentException("chain does not appear to be PEM-encoded");
        }
    }
}
