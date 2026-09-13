# The published Kyuubi image is amd64; only its architecture-independent distribution is copied.
FROM --platform=linux/amd64 apache/kyuubi:1.12.0-spark@sha256:6f63291e3b2b1d314e37d12de5f1be37bd0e9fdd44e55b050d103ffb87925e81 AS kyuubi
FROM eclipse-temurin:17-jdk-jammy@sha256:ef4374b4b6b9d813dd3f5b593a35ec9a820cfb64a55994798147cc73435a0208 AS java
FROM apache/spark:3.5.3@sha256:d0f5d63bfbbffdbec08debb4911554ba671a2fa7398374db8db314d80328f576 AS fixture
USER root
COPY --from=java /opt/java/openjdk /opt/java/openjdk
ENV JAVA_HOME=/opt/java/openjdk PATH=/opt/java/openjdk/bin:$PATH
COPY Blocking.java /tmp/Blocking.java
RUN mkdir /tmp/classes && javac -cp '/opt/spark/jars/*' -d /tmp/classes /tmp/Blocking.java \
    && jar cf /opt/spark/jars/qrow-fixture.jar -C /tmp/classes .
COPY --from=kyuubi /opt/kyuubi /opt/kyuubi
COPY kyuubi-defaults.conf /opt/kyuubi/conf/kyuubi-defaults.conf
ENV KYUUBI_HOME=/opt/kyuubi SPARK_HOME=/opt/spark KYUUBI_LOG_DIR=/opt/kyuubi/logs
RUN mkdir -p /evidence /opt/kyuubi/logs /opt/kyuubi/pid /opt/kyuubi/work \
    && chmod -R a+rwX /evidence /opt/kyuubi /opt/spark/work-dir
WORKDIR /opt/kyuubi
ENTRYPOINT []
CMD ["/opt/kyuubi/bin/kyuubi", "run"]
