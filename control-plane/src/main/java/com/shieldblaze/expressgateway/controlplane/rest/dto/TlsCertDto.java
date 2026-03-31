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
package com.shieldblaze.expressgateway.controlplane.rest.dto;

import com.shieldblaze.expressgateway.controlplane.config.types.TlsCertSpec;

/**
 * DTO for TLS certificate CRUD operations.
 *
 * @param name        the certificate name
 * @param certificate PEM-encoded certificate
 * @param privateKey  PEM-encoded private key
 * @param chain       PEM-encoded certificate chain (optional)
 * @param autoRenew   whether to automatically renew this certificate
 */
public record TlsCertDto(
        String name,
        String certificate,
        String privateKey,
        String chain,
        boolean autoRenew
) {

    /**
     * Convert this DTO to a {@link TlsCertSpec}.
     */
    public TlsCertSpec toSpec() {
        return new TlsCertSpec(name, certificate, privateKey, chain, autoRenew);
    }

    /**
     * Create a DTO from a {@link TlsCertSpec}.
     */
    public static TlsCertDto from(TlsCertSpec spec) {
        return new TlsCertDto(
                spec.name(),
                spec.certificate(),
                spec.privateKey(),
                spec.chain(),
                spec.autoRenew()
        );
    }
}
