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
package com.shieldblaze.expressgateway.core.tls;

import com.shieldblaze.expressgateway.core.configuration.tls.TLSConfiguration;
import io.netty.handler.ssl.SslContext;
import io.netty.util.Mapping;

public final class SNIMapper implements Mapping<String, SslContext> {

    private final TLSConfiguration tlsConfiguration;

    public SNIMapper(TLSConfiguration tlsConfiguration) {
        this.tlsConfiguration = tlsConfiguration;
    }

    @Override
    public SslContext map(String input) {

        try {
            SslContext sslContext = tlsConfiguration.getHostnameCertificateMapping().get(input);

            // If `null` it means, Mapping was not found with FQDN. Now we'll try Wildcard
            if (sslContext == null) {
                input = "*" +  input.substring(input.indexOf("."));
                sslContext = tlsConfiguration.getHostnameCertificateMapping().get(input);
                if (sslContext != null) {
                    return sslContext;
                }
            }
        } catch (NullPointerException ex) {
            // Ignore
        }

        return tlsConfiguration.getDefault();
    }
}
