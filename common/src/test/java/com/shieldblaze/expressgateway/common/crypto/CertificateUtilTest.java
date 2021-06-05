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
package com.shieldblaze.expressgateway.common.crypto;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

class CertificateUtilTest {

    @Test
    void parseCertTest() {
        String cert = "-----BEGIN CERTIFICATE-----\n" +
                "MIIFKzCCBBOgAwIBAgISA62BMjjm25gZ9Kldl/77JhKQMA0GCSqGSIb3DQEBCwUA\n" +
                "MDIxCzAJBgNVBAYTAlVTMRYwFAYDVQQKEw1MZXQncyBFbmNyeXB0MQswCQYDVQQD\n" +
                "EwJSMzAeFw0yMTAzMjcxMTM2MTVaFw0yMTA2MjUxMTM2MTVaMB4xHDAaBgNVBAMT\n" +
                "E3d3dy5zaGllbGRibGF6ZS5jb20wggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEK\n" +
                "AoIBAQCrxxsM7cYB+Oqps88IF0+iy3w0xGYS5u/zmBd5yWXuZkwfmpJ9M+4H+i4V\n" +
                "Yve08x/VTy6xZ6hJQr/jzJq3MEbCaPUoqWRpb0xLZCTJ3O1Gn6Qfwu9vNtC8aSe4\n" +
                "4tYYcEAstPXuj/cNjG4Dkudd1j68u8lbKBCgWvY39eGeFSNybo5pAQmkjKTJ19sF\n" +
                "AZBIS5AgjDh6CmB0eRgmMI5gCxe5JKCA3z8UANMJ5zRHNWN8VNKgneFX0csT0zww\n" +
                "JJeO6jQAn8xsDGr3VLxeYNxGMcIJ3tnD42MejxzFkJDo2oa+ffHDHxqGaZsL4LIM\n" +
                "RwjIklkrZi/6oTihLxBl9pf9FoczAgMBAAGjggJNMIICSTAOBgNVHQ8BAf8EBAMC\n" +
                "BaAwHQYDVR0lBBYwFAYIKwYBBQUHAwEGCCsGAQUFBwMCMAwGA1UdEwEB/wQCMAAw\n" +
                "HQYDVR0OBBYEFGNOFYVWWqSUAsIWQqSll5o4AleXMB8GA1UdIwQYMBaAFBQusxe3\n" +
                "WFbLrlAJQOYfr52LFMLGMFUGCCsGAQUFBwEBBEkwRzAhBggrBgEFBQcwAYYVaHR0\n" +
                "cDovL3IzLm8ubGVuY3Iub3JnMCIGCCsGAQUFBzAChhZodHRwOi8vcjMuaS5sZW5j\n" +
                "ci5vcmcvMB4GA1UdEQQXMBWCE3d3dy5zaGllbGRibGF6ZS5jb20wTAYDVR0gBEUw\n" +
                "QzAIBgZngQwBAgEwNwYLKwYBBAGC3xMBAQEwKDAmBggrBgEFBQcCARYaaHR0cDov\n" +
                "L2Nwcy5sZXRzZW5jcnlwdC5vcmcwggEDBgorBgEEAdZ5AgQCBIH0BIHxAO8AdQBv\n" +
                "U3asMfAxGdiZAKRRFf93FRwR2QLBACkGjbIImjfZEwAAAXhzr1reAAAEAwBGMEQC\n" +
                "IDJsjvNIuSt5KuJzslYw1znMfqYEW1kNB/rIouzqbViPAiB0vFOpC9NIoz64mFJa\n" +
                "XmR/CJrnlzkhCHYB03xG9FyKXQB2AH0+8viP/4hVaCTCwMqeUol5K8UOeAl/LmqX\n" +
                "aJl+IvDXAAABeHOvWxEAAAQDAEcwRQIgdmsJ9b9OngtCe8hjp479HjE74Zgif6rd\n" +
                "KelFUTCIafQCIQDhp2SHkZLW9/Q2lF7kMzAHrfFvjtTGxuiHqUEgzPTIFzANBgkq\n" +
                "hkiG9w0BAQsFAAOCAQEAnYZ7IGrId1k1qck7wuTLwFM65/qRtte9DDtajJxhnk6F\n" +
                "AHikqEH/gpzafP9ejGqLw7MMe7CW+fGWd/mGyws4DwBq9V/Y8y4JAWuFIu6V4/G6\n" +
                "3bnDZJC2195TSPwezOwVk0ydJkPTzohz0NxpxYAipO7bRRuuwuMb82xSTVd+9kgA\n" +
                "66twDexv4tB1l/F77MfgsmegTM+QpWrkqNxtfeJsOFNhs+n4hP2FQmXsnDjkCuPs\n" +
                "k9K4sHdwqn+DeWrS7k9jOwi618Ufh1Byljv+5w/N9SN2pqlTH4HMOLWUKY+/4RqI\n" +
                "iD/L8YodA6pR0kER16AITT4ttrmLhknV6nWZ8LPcJw==\n" +
                "-----END CERTIFICATE-----";

        assertDoesNotThrow(() -> CertificateUtil.parseX509Certificate(cert));
    }

    @Test
    void parseBadCertTest() {
        String cert = "MIIFKzCCBBOgAwIBAgISA62BMjjm25gZ9Kldl/77JhKQMA0GCSqGSIb3DQEBCwUA\n" +
                "MDIxCzAJBgNVBAYTAlVTMRYwFAYDVQQKEw1MZXQncyBFbmNyeXB0MQswCQYDVQQD\n" +
                "EwJSMzAeFw0yMTAzMjcxMTM2MTVaFw0yMTA2MjUxMTM2MTVaMB4xHDAaBgNVBAMT\n" +
                "E3d3dy5zaGllbGRibGF6ZS5jb20wggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEK\n" +
                "AoIBAQCrxxsM7cYB+Oqps88IF0+iy3w0xGYS5u/zmBd5yWXuZkwfmpJ9M+4H+i4V\n" +
                "Yve08x/VTy6xZ6hJQr/jzJq3MEbCaPUoqWRpb0xLZCTJ3O1Gn6Qfwu9vNtC8aSe4\n" +
                "4tYYcEAstPXuj/cNjG4Dkudd1j68u8lbKBCgWvY39eGeFSNybo5pAQmkjKTJ19sF\n" +
                "AZBIS5AgjDh6CmB0eRgmMI5gCxe5JKCA3z8UANMJ5zRHNWN8VNKgneFX0csT0zww\n" +
                "JJeO6jQAn8xsDGr3VLxeYNxGMcIJ3tnD42MejxzFkJDo2oa+ffHDHxqGaZsL4LIM\n" +
                "RwjIklkrZi/6oTihLxBl9pf9FoczAgMBAAGjggJNMIICSTAOBgNVHQ8BAf8EBAMC\n" +
                "BaAwHQYDVR0lBBYwFAYIKwYBBQUHAwEGCCsGAQUFBwMCMAwGA1UdEwEB/wQCMAAw\n" +
                "HQYDVR0OBBYEFGNOFYVWWqSUAsIWQqSll5o4AleXMB8GA1UdIwQYMBaAFBQusxe3\n" +
                "WFbLrlAJQOYfr52LFMLGMFUGCCsGAQUFBwEBBEkwRzAhBggrBgEFBQcwAYYVaHR0\n" +
                "cDovL3IzLm8ubGVuY3Iub3JnMCIGCCsGAQUFBzAChhZodHRwOi8vcjMuaS5sZW5j\n" +
                "ci5vcmcvMB4GA1UdEQQXMBWCE3d3dy5zaGllbGRibGF6ZS5jb20wTAYDVR0gBEUw\n" +
                "QzAIBgZngQwBAgEwNwYLKwYBBAGC3xMBAQEwKDAmBggrBgEFBQcCARYaaHR0cDov\n" +
                "L2Nwcy5sZXRzZW5jcnlwdC5vcmcwggEDBgorBgEEAdZ5AgQCBIH0BIHxAO8AdQBv\n" +
                "U3asMfAxGdiZAKRRFf93FRwR2QLBACkGjbIImjfZEwAAAXhzr1reAAAEAwBGMEQC\n" +
                "IDJsjvNIuSt5KuJzslYw1znMfqYEW1kNB/rIouzqbViPAiB0vFOpC9NIoz64mFJa\n" +
                "XmR/CJrnlzkhCHYB03xG9FyKXQB2AH0+8viP/4hVaCTCwMqeUol5K8UOeAl/LmqX\n" +
                "aJl+IvDXAAABeHOvWxEAAAQDAEcwRQIgdmsJ9b9OngtCe8hjp479HjE74Zgif6rd\n" +
                "KelFUTCIafQCIQDhp2SHkZLW9/Q2lF7kMzAHrfFvjtTGxuiHqUEgzPTIFzANBgkq\n" +
                "hkiG9w0BAQsFAAOCAQEAnYZ7IGrId1k1qck7wuTLwFM65/qRtte9DDtajJxhnk6F\n" +
                "AHikqEH/gpzafP9ejGqLw7MMe7CW+fGWd/mGyws4DwBq9V/Y8y4JAWuFIu6V4/G6\n" +
                "3bnDZJC2195TSPwezOwVk0ydJkPTzohz0NxpxYAipO7bRRuuwuMb82xSTVd+9kgA\n" +
                "66twDexv4tB1l/F77MfgsmegTM+QpWrkqNxtfeJsOFNhs+n4hP2FQmXsnDjkCuPs\n" +
                "k9K4sHdwqn+DeWrS7k9jOwi618Ufh1Byljv+5w/N9SN2pqlTH4HMOLWUKY+/4RqI\n" +
                "iD/L8YodA6pR0kER16AITT4ttrmLhknV6nWZ8LPcJw==";

        assertThrows(IllegalArgumentException.class, () -> CertificateUtil.parseX509Certificate(cert));
    }
}
