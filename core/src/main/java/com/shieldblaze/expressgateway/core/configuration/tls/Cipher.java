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

/**
 * List of available Ciphers Suites under OpenSsl 1.1.1g
 */
public enum Cipher {
    TLS_AES_256_GCM_SHA384,
    TLS_CHACHA20_POLY1305_SHA256,
    TLS_AES_128_GCM_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
    TLS_DHE_DSS_WITH_AES_256_GCM_SHA384,
    TLS_DHE_RSA_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
    TLS_DHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_256_CBC_CCM8,
    TLS_ECDHE_ECDSA_WITH_AES_256_CBC_CCM,
    TLS_DHE_RSA_WITH_AES_256_CBC_CCM8,
    TLS_DHE_RSA_WITH_AES_256_CBC_CCM,
    TLS_ECDHE_ECDSA_WITH_ARIA256_GCM_SHA384,
    TLS_RSA_WITH_ECDHE_ARIA256_GCM_SHA384,
    TLS_DHE_DSS_WITH_ARIA256_GCM_SHA384,
    TLS_DHE_RSA_WITH_ARIA256_GCM_SHA384,
    TLS_DH_anon_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
    TLS_DHE_DSS_WITH_AES_128_GCM_SHA256,
    TLS_DHE_RSA_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_128_CBC_CCM8,
    TLS_ECDHE_ECDSA_WITH_AES_128_CBC_CCM,
    TLS_DHE_RSA_WITH_AES_128_CBC_CCM8,
    TLS_DHE_RSA_WITH_AES_128_CBC_CCM,
    TLS_ECDHE_ECDSA_WITH_ARIA128_GCM_SHA256,
    TLS_RSA_WITH_ECDHE_ARIA128_GCM_SHA256,
    TLS_DHE_DSS_WITH_ARIA128_GCM_SHA256,
    TLS_DHE_RSA_WITH_ARIA128_GCM_SHA256,
    TLS_DH_anon_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA384,
    TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA384,
    TLS_DHE_RSA_WITH_AES_256_CBC_SHA256,
    TLS_DHE_DSS_WITH_AES_256_CBC_SHA256,
    TLS_ECDHE_ECDSA_WITH_CAMELLIA256_SHA384,
    TLS_ECDHE_RSA_WITH_CAMELLIA256_SHA384,
    TLS_DHE_RSA_WITH_CAMELLIA256_SHA256,
    TLS_DHE_DSS_WITH_CAMELLIA256_SHA256,
    TLS_DH_anon_WITH_AES_256_CBC_SHA256,
    TLS_DH_anon_WITH_CAMELLIA256_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA256,
    TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA256,
    TLS_DHE_RSA_WITH_AES_128_CBC_SHA256,
    TLS_DHE_DSS_WITH_AES_128_CBC_SHA256,
    TLS_ECDHE_ECDSA_WITH_CAMELLIA128_SHA256,
    TLS_ECDHE_RSA_WITH_CAMELLIA128_SHA256,
    TLS_DHE_RSA_WITH_CAMELLIA128_SHA256,
    TLS_DHE_DSS_WITH_CAMELLIA128_SHA256,
    TLS_DH_anon_WITH_AES_128_CBC_SHA256,
    TLS_DH_anon_WITH_CAMELLIA128_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA,
    TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA,
    TLS_DHE_RSA_WITH_AES_256_CBC_SHA,
    TLS_DHE_DSS_WITH_AES_256_CBC_SHA,
    TLS_DHE_RSA_WITH_CAMELLIA256_SHA,
    TLS_DHE_DSS_WITH_CAMELLIA256_SHA,
    TLS_ECDH_anon_WITH_AES_256_CBC_SHA,
    TLS_DH_anon_WITH_AES_256_CBC_SHA,
    TLS_DH_anon_WITH_CAMELLIA256_SHA,
    TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA,
    TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA,
    TLS_DHE_RSA_WITH_AES_128_CBC_SHA,
    TLS_DHE_DSS_WITH_AES_128_CBC_SHA,
    TLS_DHE_RSA_WITH_SEED_SHA,
    TLS_DHE_DSS_WITH_SEED_SHA,
    TLS_DHE_RSA_WITH_CAMELLIA128_SHA,
    TLS_DHE_DSS_WITH_CAMELLIA128_SHA,
    TLS_ECDH_anon_WITH_AES_128_CBC_SHA,
    TLS_DH_anon_WITH_AES_128_CBC_SHA,
    TLS_DH_anon_WITH_SEED_SHA,
    TLS_DH_anon_WITH_CAMELLIA128_SHA,
    TLS_RSA_PSK_WITH_AES_256_GCM_SHA384,
    TLS_DHE_PSK_WITH_AES_256_GCM_SHA384,
    TLS_RSA_PSK_WITH_CHACHA20_POLY1305_SHA256,
    TLS_DHE_PSK_WITH_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_PSK_WITH_CHACHA20_POLY1305_SHA256,
    TLS_DHE_PSK_WITH_AES_256_CBC_CCM8,
    TLS_DHE_PSK_WITH_AES_256_CBC_CCM,
    TLS_RSA_PSK_WITH_ARIA256_GCM_SHA384,
    TLS_DHE_PSK_WITH_ARIA256_GCM_SHA384,
    TLS_RSA_WITH_AES_256_GCM_SHA384,
    TLS_RSA_WITH_AES_256_CBC_CCM8,
    TLS_RSA_WITH_AES_256_CBC_CCM,
    TLS_RSA_WITH_ARIA256_GCM_SHA384,
    TLS_PSK_WITH_AES_256_GCM_SHA384,
    TLS_PSK_WITH_CHACHA20_POLY1305_SHA256,
    TLS_PSK_WITH_AES_256_CBC_CCM8,
    TLS_PSK_WITH_AES_256_CBC_CCM,
    TLS_PSK_WITH_ARIA256_GCM_SHA384,
    TLS_RSA_PSK_WITH_AES_128_GCM_SHA256,
    TLS_DHE_PSK_WITH_AES_128_GCM_SHA256,
    TLS_DHE_PSK_WITH_AES_128_CBC_CCM8,
    TLS_DHE_PSK_WITH_AES_128_CBC_CCM,
    TLS_RSA_PSK_WITH_ARIA128_GCM_SHA256,
    TLS_DHE_PSK_WITH_ARIA128_GCM_SHA256,
    TLS_RSA_WITH_AES_128_GCM_SHA256,
    TLS_RSA_WITH_AES_128_CBC_CCM8,
    TLS_RSA_WITH_AES_128_CBC_CCM,
    TLS_RSA_WITH_ARIA128_GCM_SHA256,
    TLS_PSK_WITH_AES_128_GCM_SHA256,
    TLS_PSK_WITH_AES_128_CBC_CCM8,
    TLS_PSK_WITH_AES_128_CBC_CCM,
    TLS_PSK_WITH_ARIA128_GCM_SHA256,
    TLS_RSA_WITH_AES_256_CBC_SHA256,
    TLS_RSA_WITH_CAMELLIA256_SHA256,
    TLS_RSA_WITH_AES_128_CBC_SHA256,
    TLS_RSA_WITH_CAMELLIA128_SHA256,
    TLS_ECDHE_PSK_WITH_AES_256_CBC_SHA384,
    TLS_ECDHE_PSK_WITH_AES_256_CBC_SHA,
    TLS_SRP_DSS_WITH_AES_256_CBC_SHA,
    TLS_SRP_RSA_WITH_AES_256_CBC_SHA,
    TLS_SRP_WITH_AES_256_CBC_SHA,
    TLS_RSA_PSK_WITH_AES_256_CBC_SHA384,
    TLS_DHE_PSK_WITH_AES_256_CBC_SHA384,
    TLS_RSA_PSK_WITH_AES_256_CBC_SHA,
    TLS_DHE_PSK_WITH_AES_256_CBC_SHA,
    TLS_ECDHE_PSK_WITH_CAMELLIA256_SHA384,
    TLS_RSA_PSK_WITH_CAMELLIA256_SHA384,
    TLS_DHE_PSK_WITH_CAMELLIA256_SHA384,
    TLS_RSA_WITH_AES_256_CBC_SHA,
    TLS_RSA_WITH_CAMELLIA256_SHA,
    TLS_PSK_WITH_AES_256_CBC_SHA384,
    TLS_PSK_WITH_AES_256_CBC_SHA,
    TLS_PSK_WITH_CAMELLIA256_SHA384,
    TLS_ECDHE_PSK_WITH_AES_128_CBC_SHA256,
    TLS_ECDHE_PSK_WITH_AES_128_CBC_SHA,
    TLS_SRP_DSS_WITH_AES_128_CBC_SHA,
    TLS_SRP_RSA_WITH_AES_128_CBC_SHA,
    TLS_SRP_WITH_AES_128_CBC_SHA,
    TLS_RSA_PSK_WITH_AES_128_CBC_SHA256,
    TLS_DHE_PSK_WITH_AES_128_CBC_SHA256,
    TLS_RSA_PSK_WITH_AES_128_CBC_SHA,
    TLS_DHE_PSK_WITH_AES_128_CBC_SHA,
    TLS_ECDHE_PSK_WITH_CAMELLIA128_SHA256,
    TLS_RSA_PSK_WITH_CAMELLIA128_SHA256,
    TLS_DHE_PSK_WITH_CAMELLIA128_SHA256,
    TLS_RSA_WITH_AES_128_CBC_SHA,
    TLS_RSA_WITH_SEED_SHA,
    TLS_RSA_WITH_CAMELLIA128_SHA,
    TLS_RSA_WITH_IDEA_CBC_SHA,
    TLS_PSK_WITH_AES_128_CBC_SHA256,
    TLS_PSK_WITH_AES_128_CBC_SHA,
    TLS_PSK_WITH_CAMELLIA128_SHA256
}
