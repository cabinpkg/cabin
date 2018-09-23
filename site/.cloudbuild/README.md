## .cloudbuild

### Creation
> https://github.com/ahmetb/gke-letsencrypt/blob/master/10-install-helm.md
```bash
kubectl create serviceaccount -n kube-system tiller
kubectl create clusterrolebinding tiller-binding \
    --clusterrole=cluster-admin \
    --serviceaccount kube-system:tiller
helm init --service-account tiller
helm repo update

kubectl -n kube-system get po | grep tiller
```

> https://github.com/ahmetb/gke-letsencrypt/blob/master/20-install-cert-manager.md
```bash
helm install \
    --name cert-manager \
    --namespace kube-system \
    stable/cert-manager
```

> https://github.com/ahmetb/gke-letsencrypt/blob/master/30-setup-letsencrypt.md
```bash
kubectl create secret generic prod-route53-credentials-secret --from-literal=secret-access-key=
curl -sSL https://raw.githubusercontent.com/poacpm/poac.pm/master/.cloudbuild/issuer.yaml | \
    sed -e "s/email: ''/email: $EMAIL/g" | \
    sed -e "s/accessKeyID: ''/accessKeyID: $AWS_ACCESS_KEY_ID/g" | \
    kubectl apply -f-
```

> https://github.com/ahmetb/gke-letsencrypt/blob/master/40-deploy-an-app.md
```bash
kubectl apply -f deployment.yaml
kubectl apply -f service.yaml

gcloud compute addresses create poac-pm-ip --global

kubectl apply -f ingress.yaml
```

> https://github.com/ahmetb/gke-letsencrypt/blob/master/50-get-a-certificate.md
```bash
kubectl apply -f certificate.yaml

kubectl describe certificate
```

> https://github.com/ahmetb/gke-letsencrypt/blob/master/60-start-serving-https.md
```bash
kubectl apply -f ingress-tls.yaml
```


### Cleanup

> https://github.com/ahmetb/gke-letsencrypt/blob/master/99-cleanup.md
