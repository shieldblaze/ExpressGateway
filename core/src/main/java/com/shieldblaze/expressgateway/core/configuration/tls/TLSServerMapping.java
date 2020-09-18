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

import java.util.HashMap;
import java.util.Map;

/**
 * TLS Server Mapping contains mapping for Hostname and {@link CertificateKeyPair}
 */
public final class TLSServerMapping {
    final Map<String, CertificateKeyPair> certificateKeyMap = new HashMap<>();

    /**
     * Create new Instance of {@link TLSServerMapping}
     */
    public TLSServerMapping() {
        // Empty Constructor
    }

    /**
     * Create new Instance of {@link TLSServerMapping} along with default {@link CertificateKeyPair}
     *
     * @param certificateKeyPair Default {@link CertificateKeyPair}
     */
    public TLSServerMapping(CertificateKeyPair certificateKeyPair) {
        certificateKeyMap.put("DEFAULT_HOST", certificateKeyPair);
    }

    /**
     * Add Mapping
     * @param hostname Hostname to which {@link CertificateKeyPair} will be mapped
     * @param certificateKeyPair {@link CertificateKeyPair} to be mapped
     */
    public void addMapping(String hostname, CertificateKeyPair certificateKeyPair) {
        certificateKeyMap.put(hostname, certificateKeyPair);
    }
}
