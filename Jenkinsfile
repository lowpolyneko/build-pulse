pipeline {
    agent any
    stages {
        stage('prepare') {
            copyArtifacts projectName: currentBuild.fullProjectName, selector: lastCompleted, exclude: 'build-pulse', optional: true
        }
        stage('build') {
            agent {
                docker {
                    image 'rust:latest'
                    reuseNode true
                }
            }
            steps {
                sh 'cargo build --release'
            }
        }
        stage('deploy') {
            steps {
                sh './target/release/build-pulse -o report.html'
            }
        }
        stage('package') {
            archiveArtifacts artifacts: 'target/release/build-pulse,report.html,data.db,config.toml'
        }
    }
}

// vim: ts=4:sw=4:expandtab:
