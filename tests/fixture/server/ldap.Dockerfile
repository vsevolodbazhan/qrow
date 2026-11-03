FROM osixia/openldap:1.5.0@sha256:18742e9c449c9c1afe129d3f2f3ee15fb34cc43e5f940a20f3399728f41d7c28
COPY ldap-ready.sh /usr/local/bin/qrow-ldap-ready
RUN chmod +x /usr/local/bin/qrow-ldap-ready \
    && grep -Fq 'while [ ! -e /run/slapd/slapd.pid ]; do sleep 0.1; done' /container/service/slapd/startup.sh \
    && sed -i '/while \[ ! -e \/run\/slapd\/slapd.pid \]; do sleep 0.1; done/c\    /usr/local/bin/qrow-ldap-ready' /container/service/slapd/startup.sh
COPY users.ldif /container/service/slapd/assets/config/bootstrap/ldif/custom/50-qrow.ldif
CMD ["--copy-service"]
