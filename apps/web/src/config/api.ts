import axios from "axios";

const apiInstance = axios.create({
  headers: {
    "Content-Type": "application/json",
  },
  withCredentials: true,
  timeout: 120000, // 2 minutes timeout for API calls
});

export default apiInstance;
