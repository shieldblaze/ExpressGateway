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

import java.security.PrivateKey;

import static org.junit.jupiter.api.Assertions.assertEquals;

class PrivateKeyUtilTest {

    @Test
    void ecPrime256V1PrivateKeyTest() {
        String key = "-----BEGIN EC PRIVATE KEY-----\n" +
                "MHcCAQEEIF0YzKGMZju3/vX3eFDHpBAKhWlKbqC1ar/X0PAT8xDnoAoGCCqGSM49\n" +
                "AwEHoUQDQgAEgx/PNAeV2Ew1+rt6axxjwjBJDRJiiFF6CE77LRRXdxxGwXdtHD2j\n" +
                "DRvsrKzHBs9Wa0Lq0TxsEpPoxqbjZ+upqA==\n" +
                "-----END EC PRIVATE KEY-----\n";

        PrivateKey privateKey = PrivateKeyUtil.parsePrivateKey(key);
        assertEquals("ECDSA", privateKey.getAlgorithm());
    }

    @Test
    void ecSECP384R1PrivateKeyTest() {
        String key = "-----BEGIN EC PRIVATE KEY-----\n" +
                "MIGkAgEBBDDCliFtEXt8fvgn7W0/sFpw7GmVTmqx1BVnS+RYEeIgpy/j+zNYIUjE\n" +
                "mgW9vussoxOgBwYFK4EEACKhZANiAASYEXqpmPrM1xxXg2XfJ3YHUJIhVtR7Hc3u\n" +
                "BzVpmr8L20E6ph0ikSCl9Q9768GHTyrrSEZaR8p3uiV/N/TZw3so0iPQxgVZyVB0\n" +
                "voYAvfYqEL8bdzvuhARxMIBP5Is2QLA=\n" +
                "-----END EC PRIVATE KEY-----\n";

        PrivateKey privateKey = PrivateKeyUtil.parsePrivateKey(key);
        assertEquals("ECDSA", privateKey.getAlgorithm());
    }

    @Test
    void ecSECP521R1PrivateKeyTest() {
        String key = "-----BEGIN EC PRIVATE KEY-----\n" +
                "MIHcAgEBBEIAx+Of+Vq+nIEVR6d+hMTJTNXRJatHLWrp8b1XIAEk+lYAwn9hi+fW\n" +
                "OqnAGyAGoerC6NM8y+HBftPPML36w0gW7DygBwYFK4EEACOhgYkDgYYABAGUS9UV\n" +
                "Xf1DdFMKqiYuxanj6dryTKPgvkBiuY6URmkjeST5WS5ZCKAr3Q+W1b91kh9FFLqH\n" +
                "KxEkpIxiQz4sjYd2lAB4ehuVGE2DTIzCTrMSd1cxb+N5hKCm1NP3Yjb5EITzzMWU\n" +
                "mAAK4vt0gdYtGfkQkPdj7iD7txtKLfS36FohmhPgfg==\n" +
                "-----END EC PRIVATE KEY-----\n";

        PrivateKey privateKey = PrivateKeyUtil.parsePrivateKey(key);
        assertEquals("ECDSA", privateKey.getAlgorithm());
    }

    @Test
    void rsa2048PrivateKeyTest() {
        String key = "-----BEGIN RSA PRIVATE KEY-----\n" +
                "MIIEpAIBAAKCAQEAn1mavkQeGd/8Jh0Oo2S59uhIPRfgK2cLE4nLLIYMFEuhm1gg\n" +
                "ijL/5mDingC4moOwWm0xPmA4NT+fSmbWp+GngTUrxKksdiNDhuBTJA1QMfeHm7hi\n" +
                "wpxrM5vp1UX5gb95OXfWO84qACNLvm8hikRm2nWmEtA9ftdgiwB5wkwbarHYmQy0\n" +
                "y6ogz3ZcBE3VNNtN/coPzkuPKhK9VETBogtH38uLmkbMZTROpSMYe8kpYUO2mjhm\n" +
                "WUB+ienI65azxbXWRhjdB0GXygbj3XtXJvJPN6+CpgKGO+9GfpT6PkUsSqgXSPpm\n" +
                "PQKtFxHdvlRuUenUb7Ems2INdjqTKgzhrbICNwIDAQABAoIBAAI2bk2iCID3eTrW\n" +
                "QHPIXESvaQFgKX6wyZiV2zpdCiHmAvJzQNBqcN80DTGAdQ/AMFyxE3P4Rg+HOKEB\n" +
                "2I0wNvE2Wcs7RiDF0JQ43S6s+KYY98sTvltRbLOkwJRziklg7T/9B/6AmFi0JMMa\n" +
                "K+8NkBgMdifguFngh7imYwptoBvVIr46u4vnS3TnE0LfHjcZbgRt8vD+rwcx1cjz\n" +
                "vqa9YFu4czmUyJFsmGEae+RjtC2hZvTMIQI/yBdeQjn57tV1l9ofHDl8NLckLCC6\n" +
                "GZJC+e/0k2A7Rfvho9ASo8uhlsz7KDOCiIcsBHbp5X+giX4GWnPWvCewPAP9WWdz\n" +
                "LrjKFXECgYEA0DMogxjJePQ4OPuvaScggKsQiTJY4sK2+09B+XMekhphYkoTNWBb\n" +
                "Im+5Z7RYvM+IOGvH2xN6qeR1LvKdGUkQgICMlHQh5aahZS3+xx1h4StWbmFRgX4R\n" +
                "joC9W1HVL17ufZGZklUFUOY8LWjDqdimzQPKG3uyyL+I4xWeuvJTj1UCgYEAw+9S\n" +
                "iLqEkCuM3CgFFDiXCrCx02zKuEDbM3Nri0g8S6/Om4UWSnEcLotA5ovpy0eVEYlI\n" +
                "FrNDxAGhKfENoWjVWlpgTOiGChfk20SFex46SlcIz2ZEXnBT9WZdkMgKB3NVHk+l\n" +
                "dXdPC3jAMU/t2bIhsvjKWd4doOxSxcw/Z3Fv01sCgYEApV3c8bPAYtfnyCrwel7f\n" +
                "jBNTgQWCYCo0WIvtZQqw328gpocblqu/9ywyYVJ3oRBdrCK/jRx9s2+IPA/sA+dq\n" +
                "ugZgopFTUyr0yn6r/M8zrTtU3TtjF61gmIVc4amv4H5Qg2AgUIBDRqx4Y8DwmHlC\n" +
                "k1hNMWMg8B5hxayodOiAwjkCgYEAvUPOi/jv2HvZA0k9Lr1DWbY48Cwk3jr4Awk5\n" +
                "Fz/dzpaykxPJ5nrAaE1nkcwROKWa32em1RaxHQMd9O++5O3pOfAXGfN6lwFhtlTI\n" +
                "Q5d9YxYTkpQM8tS6pVAOja5N0cicrjztbTykhEZOENROw30IhGNCw/CE2k+t3Rdh\n" +
                "H8E57gUCgYAWaf9LSw2DDvxJihiAOjGKpwQ8joMDHkHRZ2dAn8BSeW5s+oh/aNxm\n" +
                "Rpje/lpydiWOm6/OhxhYPeXdb4HMHVdPLpV7vCueELi2zO9yQdJNMu4H4gpDYXCX\n" +
                "4U7tzuk69aBVxx1FKK+hTfPN+ZJc7XPrI3h4mffCU1Zg12C3+brACg==\n" +
                "-----END RSA PRIVATE KEY-----\n";

        PrivateKey privateKey = PrivateKeyUtil.parsePrivateKey(key);
        assertEquals("RSA", privateKey.getAlgorithm());
    }

    @Test
    void rsa3072PrivateKeyTest() {
        String key = "-----BEGIN RSA PRIVATE KEY-----\n" +
                "MIIG5AIBAAKCAYEAw1S8eWTVm8rZj3BzVrrdMQH9cdFmOClBsIEnC1tF5T5OKzK4\n" +
                "t+dHbqv5RzncwL3KvjMbNMXQV4oScqVkG38I9/MyvHP/tXnCCSwfO2ZFAxakej3M\n" +
                "5Q8vQio0DsW4igu6RcH7yl7YmS+bRkXokzz1Xe7+KH37Cd/DR9tmI8yFcPe/HPVk\n" +
                "tfIyAEUNGI2bQ0x+9oMgHwk0Vhxrn1PhVd6HrDCCWmACSdQt23o7ZodYcP1xC+Ov\n" +
                "brrA5hw1jz0oJb/0wePjLr0KIk2awH++NA/pTJ+bNOKh9atYSusBE2co/G6tXvK3\n" +
                "K0cNVpn61UwgkYeEjMQPipJzEupSPpIfklhBCCByfIMzWGKgafEU7gUuo9KeK7o8\n" +
                "U1z2RxVYZ2JBUcJKqz1wMdi3JU6hEMcJEMMddpZ4QLPmKa2OmbdsHaLmVww2HRVo\n" +
                "VoLJgT0Fdy3sRRWMCl3LxUivvwMO9I9/qQBOL3LX56ZVwg1fj9gN4ucE+/8GMFRT\n" +
                "y49o9juid4v4naIzAgMBAAECggGBAKekdyoc37QZMgYItgTu1d403gesd3Wl+wn4\n" +
                "nsEBcgihI8exfZXguo8CrCx0PcFyYqpBBI2TZQ9sog6hYjyzF8hugtH5ILLpMB5d\n" +
                "LuT1Di/rY/jCR7MkCCRaQlmXWp2oGRO7vFVgd7dpg3OZllWgENqwvpOUJDvCP0DR\n" +
                "zWTWKITfLLg26Fu75SwUF3xGNMZaxfDec1gPt0pclAWnoRLorTqcW5QjKHOi1kSz\n" +
                "MH37lW1MJ5TTsgJv6BTWiyVHkMhtaKx//JFYa/0RdVALBNlA3jckoJMh2mVkMadQ\n" +
                "CZLV7YemtIlxnxy2o6yiVuoN5z7HMH0eJL/4tkmJxjprmhJuw92frua877WVAoVw\n" +
                "panuynU7tNU2fKyF511QejnQPvWrIvcW3hYh4u0kx9j5/Si+JXsvLLG/InOGMjC3\n" +
                "/e7Kv1uKg1p71qw7b7o7t9ZE8gpacSzozURkJSeHQwghLzQiRxapmqbwJ9Rcq7XW\n" +
                "DIgVbK8pJ6J7mIlnN6BNZbT+0AC5SQKBwQDkJgbj1btUesBw/A3d/rAmR6ob2a+5\n" +
                "3XK2EqQzqxqq2cux6Sm+ZrTpurkZDytaLHNtevSyW6yjWa38yXtriFvS+b9hzToZ\n" +
                "i8djqYjVG13Iftw1g9rXiRmHh567wI2r8ksQfwKksQZrfy62LivlJX+bRPFcPkag\n" +
                "+K8t8Qxa3qMAUm0C94tftVf4eGHD7CE0grmsGjuIlDtq2Dg3ZKsh59cNLMzjX2Ui\n" +
                "xd1vM7aH4dvxfmnr5GQvjrT22q9QBwMDHRUCgcEA2y0dDo156q+xHAX/xjfuLN7y\n" +
                "AS3qGpFPGQNqIMoTtZsIvCbR1c3soA56qESB2G0ZH0FglHSzHmQuNlCreqhqYOMe\n" +
                "ZWK5By6Y7ik9Q+YYegu5HBiwMwaraRxHwytKSxlZiJytyN5x9BLUW/tCUPsdM7wQ\n" +
                "yP9T+oFxmOMYPeetK4sGgkcvJcWhBkrc9NDRb24+olyDXXIJ38TlIhHgzkpK5kia\n" +
                "oK5C4rNoLGKzLtz1klfDUKbruPf7iuCqUmLeqGQnAoHAL0EJEEuGf1rlXQF3xdEo\n" +
                "nuUdAKO31+FcDwYRaHo6DcUKgZDvinYvZnG8QMp5ijXGuphK8l42habfeIoqu/0E\n" +
                "N9BuqU0eiYgABk5o/uqqJArShWsH+rh0xzN51x1sun52ubX00DOyRrWS8Tzi7pUz\n" +
                "tu8ypo5nhpO7hOJ2UqPmUvy/g2vOPEaNL/OPHEteHUguOM0+I23AWMLr8d0x7NXe\n" +
                "HuZ2kWmCww8EbDHjzoUjTwOF4MvvTEJcjPyCbyrkntJ5AoHAFcSkRya7/hgXAg6C\n" +
                "ecBiUmiOJpnVz2+xKG3TY5BOZtIQCwfb/V0nbDoj6oRrVQB450bJ/dSdWZ5fjJt0\n" +
                "fIkvj5HfGfi1IcZ5/+VupUi8E5sIdobpMRgvfBAH/JVXGqBY6R2OkQ2uyav5FW2e\n" +
                "B4b5PoMmM6BQSegDTUj4xmU1KMb1DYleYGUBeiuDSHlY47VSWTPRBD3oRyY6D6kw\n" +
                "56wvRjHd0amdEQD6jrX6z+O4LCG1T1RUwxk2DXQE1ovlS1ovAoHBAKvbtIyu2ddp\n" +
                "A8IgCcrr4udrEIAN0N1/vA2mJNxzWyinftlFLZYPzYCEZWia4uMZKFoh8xe9X9uS\n" +
                "vWUZk22g7/DXY8aP6fGNHEKTQ1QQvnVFSfEvUjCzNAM+7Ht+ir9dgXVxqxtCvyH1\n" +
                "cXY9hi4xQn9Ief8FC4WiOoOaSGBMvjYOqq3DwcpzHyHE10lH2XHCmJmjWi1XGBGv\n" +
                "fvyTIVRCDAdgwggaBqZ7x62bzrPJ519kg1v+TstxR7eZxsUe1pxgsw==\n" +
                "-----END RSA PRIVATE KEY-----\n";

        PrivateKey privateKey = PrivateKeyUtil.parsePrivateKey(key);
        assertEquals("RSA", privateKey.getAlgorithm());
    }

    @Test
    void rsa4096PrivateKeyTest() {
        String key = "-----BEGIN RSA PRIVATE KEY-----\n" +
                "MIIJKAIBAAKCAgEAsNal2qNAyTU9pjjiEcollRPbDyGQQwEVaYvwZoVp8vP63xgr\n" +
                "Wax8BxD3JSs6zoPq79Mj7DRcAHgwsq4UlS801D2BxNry1XUvXlLNbHX9BaXJmPVX\n" +
                "6+MJy0U8+hjVIO7xUH5ECQhb6HVkUxDxe29pTSULIpn+CVtiH0MqKf51ITUd3wf0\n" +
                "1T6G4l3g6fHhautMQKhewUNjAPtLZEVP/i9QxPnpCivZcHls5Lnd7Ca3ZGur3+zH\n" +
                "U+6wzeHILs8qj1yfmXVdARI+xHGcO0JlsADuo9+SRXpKha5SQougFtp7Rerr88y/\n" +
                "bBuFimD+ByPHJuaEjrjg+QV4mfBcgmvVTIndG4h5lzbj2FX++MfzXjU3dsEH4oqd\n" +
                "PxW/p10kik6NqzrM0ZCxQD/DJ6JK4OABHaRLO8SuGUjziI9HyJ5L/V1ccmeljSG5\n" +
                "y5eselsI5pRGCIBtJYT6qVKhqcWCCaaIhS1NpFr1LEeCNQYvcCUAy4NBHjg/xyy6\n" +
                "DSy2Sq+OBTLXCy3i5Hyunc5gULnkiFtxJEd/j5MAA154Paqxkd8jIV3wk2h9/Gck\n" +
                "29Os/9Otzpu/aeQofOqCJgHtaVwebHDshvvFXu3IHMo2H3LfHibxi1u04KQkiGho\n" +
                "F8tAEC73H/p/rY0XWH6QkBQMsGHS8z9BwAaaUPsCCnT8LM0ij+07FPSSHpECAwEA\n" +
                "AQKCAgA8oBoULsvTL1GHXxECEE96IGiFc3hFwGVa3gL44txD0qk7OsoB4ERVF9fj\n" +
                "AkMS8d7lgXlbTUgNUSdA2rVrv9dYvA11M6r0y0wGBlUuzfSEryXCLrqJwDhnW8Ff\n" +
                "7IuE0uYgNmmUvyzjMPvIDpL7QmLTc9OxdSHGi3HETf5yjy4QyGkJQW0Kfnk3uf+V\n" +
                "tLsXMLvfntl9YGDcwUpDgg++kPIb1aGzPv04tihC8gXHJC7TWqZ+Cmr7t2Ud8D30\n" +
                "7kklBRgiQD42U875AgoRtt2tzWQDAm7fKuHJms3QypWDwDtq6PMjjhpCMd1CZ6Yv\n" +
                "RHDOQVFIrNFUDjAedm0AuX9S0iDe8SRDv7S5Zqolrze2SBH8x8igYnyLfBAFcmTf\n" +
                "fIwG8/KDvHECbTCHUet9Uf7+YEVBazcCe1C41OGLE7pChPZEXUaKCbrXYdtbNc5b\n" +
                "yHjmlC10kNXxMtdFjY7n6ca8eaeMcMEvHR7wf+9xQlG4mGzGIth1RIjsAHFLuUcG\n" +
                "rbFWl3WST9NaSjUVuvSpeDcrVWjS3ZAK2JISD6SwixUz8DNfN1jIsAlq4CfatCgk\n" +
                "slfOwob6c/Qrn+fZP3x+z+ULfmiWmR1J6J3GqAFN62GppGTpRaj5GxLdyt8t7hy5\n" +
                "AcKBWVnxggwyrN5NqtupL3QopwkT3MtvSYn4rqQJkRy7vehJAQKCAQEA6DhlPa6e\n" +
                "zBzJkJkuW4iXMJQU9GB1w19a3/TZNfSg6UvpO9OBX+j3BPDtUX8WI7bnThkfKg3D\n" +
                "KS1TpmVUtAKGOpzvD3XwafC8HeOxNCm/r9cYYRfwcYvOsOLr00Dm2GS+3XBJh7Ok\n" +
                "TjlAY1OTkPJ4Hu98x85iypeQXqkHz2oQJIdzgqtmqV/koE2itxqOlPoaA3bfaamZ\n" +
                "LWfeSXRMXCNy3cGAnwfk7fu8aIyRqOGhjTbnloixL5h90B6pedtB5Od+KWoDspgb\n" +
                "OQdGM6F9UUfUlGFBGd9e+8jWr7AnIR2UXq7EgnrIqZ3xJTdXUQ0CbzKOLelkzuHf\n" +
                "IDSyqNwr8CEj6QKCAQEAwvJuCvolT1LYGuWh1pSz+TcjvEhixN5YOFcX8d0i7f6c\n" +
                "DJ6ucSkHy9o18aHKnwdA0ItAAfdFREJKbnlu7PobWiqUUKYDmLbmYMSRxWsBP5Ap\n" +
                "31s/Ub5kHi/wptvOReo9Zx9vanch2F3nXKV7pdxcXIIx6Jwszlkb0bTRCOlmUC4q\n" +
                "C2dSVeloOb0hYcpC+G7ZtZqjD4EWgDgPvegWKiTSZNEssrbLrF9UsQ5zeYmXXU30\n" +
                "XbKi2jjsX/PuZP+MPVT02L3F5PxpnLKhtQ+NoUDRyUbezPZGziIIt1nZvPMKO+g0\n" +
                "I30jDopWTscK31U8KjszQt9bcKxlAN8vvxScmhzEaQKCAQBdiYnOJK42DUprgigY\n" +
                "Gpa7rIocPVZyEdCq8RclEppWHoud2337Qf1t8hXFg+lJDX1yCdBxwgVgaq+NELfj\n" +
                "ojirF1d75MeoBJ3hdDuGhWJ+06cwRNJHCkeBHIZdG4FgnIP88iPME2IVWB5FY/7G\n" +
                "ncQgwdqDKPDGJfKzDmbk9xX1gNHYSm4Zv1R59YubMYlJHMyppJItH3FhxrrU35F6\n" +
                "c5TrGexTInzmF0Y23pg1bF0EYp86FWk5gLT8xb0CJn0OVOiOifNfVsFwYSu31E6E\n" +
                "FOjds6bjYwQBa05+iffY8O84jDD/VbHKEKJ3mSkErrbST4zRlXdTlcuoT1G/jp/s\n" +
                "I40xAoIBADRqZIYVDfUPDEXnGiWM4/sNBVG5kLzoH4Y+fJSuZZbiD2khPTv10T/R\n" +
                "UxG987HgjF/GIRamOnyI6mRbyCR1duc+bZRlnq/v9W9tSthu1e4WP/vrF9JNw7OO\n" +
                "JkFm9kY8HfhdLmLM10/Kp8t0PxOwdTD2XJ7zZuSwdtdiq5We08CZOPrJ9AxfboOf\n" +
                "w5r8fBc6DtVSV6dyiO8+o5TnExaPwfYTe9YtagPVufrFLO0vvn/61spenoTYK039\n" +
                "U0ranwVak163X78a0var3OjG3sjNmdppcLxhN1ZzNi6+PNod6tGCtIoaOlPNhDk4\n" +
                "MUctLrkYI8dGMNrRr3KVj8vrqdOSCokCggEBANQO/menC+MzTXqNKPwv+V30rZQl\n" +
                "Od2SxatHRsPYhXKP9Rnl01yw5sWcwBhTXZvfIQO8euh7b87gZ7WDKzxXGRHCVjjx\n" +
                "D7MeTov+tB/0uYyLYfUh8yjc55mbrCckAi4kdLZ94RDTA/J9WSnwqqxE2GmvMfyv\n" +
                "vGfeWujmmngoOnvLEXKbwTHT4W1ZPL7/Nk9RM6rGzL9HU1MrXOKV1JhFcat9LoTD\n" +
                "Pvqty+N/zxCr4zN92rFHISOspK4tPBwNk1RiWJr9c+8Xi6AAcoW6s7rpcYDxGfQ9\n" +
                "J2hNZD4NNQZLrIWh6seICnuvdmCOOYQbwEB9sAB9PbNOkRvWXV0WkuEQgtU=\n" +
                "-----END RSA PRIVATE KEY-----\n";

        PrivateKey privateKey = PrivateKeyUtil.parsePrivateKey(key);
        assertEquals("RSA", privateKey.getAlgorithm());
    }
}
